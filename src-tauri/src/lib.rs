//! The MDMusic desktop app: Tauri commands on top of `mdm-engine` and
//! `mdm-library`.
//!
//! The UI calls the commands below, listens for `player://…` events
//! forwarded from the player thread, listens for `volume://changed` when the
//! device's Windows volume changes and `output://…` while a device that went
//! away is waited for, and loads cover art through the `cover` URL scheme.

mod media_controls;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mdm_engine::{
    Command, DeviceInfo, DeviceVolume, EqSettings, Event, OutputMode, Player, Repeat,
    VolumeChange, VolumeWatch, WasapiBackend,
};
use mdm_library::{Lyrics, Playlist, PlaylistStore, TrackDetails, TrackMeta};
use serde::{Deserialize, Serialize};
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State};

use media_controls::{Button, MediaControls};

#[derive(Debug, Clone, Serialize)]
struct OutputSettings {
    /// `None` follows the Windows default device.
    device_id: Option<String>,
    /// The mode in use.
    mode: OutputMode,
    /// The mode the user picked. It differs from `mode` while exclusive is
    /// unavailable, and comes back when a device that allows it is chosen.
    wanted: OutputMode,
    /// The device has a volume control that works in exclusive mode. Without
    /// one, the volume would be stuck at full in exclusive mode, so shared
    /// mode is used instead.
    exclusive_available: bool,
    /// The device was at full volume when it was chosen (or at launch), so it
    /// was turned down to `SAFE_LEVEL` before anything played.
    volume_lowered: bool,
    /// At launch: the name of the saved device, which was not connected, so
    /// the Windows default is used for now. The saved choice is kept.
    missing_device: Option<String>,
}

/// At or above this, a device just chosen counts as being at full volume.
/// Windows often puts USB DACs back to 100% (on replug, driver reload or
/// reboot), which is far too loud for headphones on a DAC with real gain.
const FULL_LEVEL: f32 = 0.995;
/// What a device found at full volume is turned down to.
const SAFE_LEVEL: f32 = 0.10;

impl OutputSettings {
    /// Settings for `device_id`, falling back to shared mode when the device
    /// has no adjustable hardware volume (or its volume cannot be read).
    ///
    /// With `guard_full`, a device at full volume is first turned down to
    /// `SAFE_LEVEL`, before the stream opens.
    fn resolve(device_id: Option<String>, wanted: OutputMode, guard_full: bool) -> Self {
        let mut volume = mdm_engine::get_volume(device_id.as_deref());
        let mut volume_lowered = false;
        if let Ok(v) = &volume {
            if guard_full && v.level >= FULL_LEVEL {
                let change = VolumeChange { level: Some(SAFE_LEVEL), muted: None };
                match mdm_engine::set_volume(device_id.as_deref(), change) {
                    Ok(lowered) => {
                        volume = Ok(lowered);
                        volume_lowered = true;
                    }
                    Err(e) => tracing::warn!("could not turn down a device at full volume: {e}"),
                }
            }
        }
        let exclusive_available = volume.map(|v| v.hardware).unwrap_or(false);
        let mode = if exclusive_available { wanted } else { OutputMode::Shared };
        OutputSettings {
            device_id,
            mode,
            wanted,
            exclusive_available,
            volume_lowered,
            missing_device: None,
        }
    }

    fn backend(&self) -> WasapiBackend {
        WasapiBackend { device_id: self.device_id.clone(), mode: self.mode }
    }
}

/// The output the user last picked, kept in `output.json` between launches.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedOutput {
    /// `None` follows the Windows default device.
    #[serde(default)]
    device_id: Option<String>,
    /// For telling the user which device is missing.
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default = "exclusive")]
    mode: OutputMode,
}

fn exclusive() -> OutputMode {
    OutputMode::Exclusive
}

impl Default for SavedOutput {
    fn default() -> Self {
        SavedOutput { device_id: None, device_name: None, mode: OutputMode::Exclusive }
    }
}

fn load_saved_output(path: &Path) -> SavedOutput {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_output(path: &Path, saved: &SavedOutput) {
    let result = serde_json::to_vec_pretty(saved).map_err(text).and_then(|json| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(text)?;
        }
        std::fs::write(path, json).map_err(text)
    });
    if let Err(e) = result {
        tracing::warn!("could not save the output device: {e}");
    }
}

/// The output to start with: the saved device if it is connected, else the
/// Windows default (keeping the saved mode).
fn startup_output(saved: &SavedOutput) -> OutputSettings {
    let mut missing = None;
    let device_id = match &saved.device_id {
        Some(id) => {
            let connected = mdm_engine::list_devices()
                .map(|list| list.iter().any(|d| &d.id == id))
                .unwrap_or(false);
            if connected {
                Some(id.clone())
            } else {
                missing = Some(saved.device_name.clone().unwrap_or_else(|| "The saved output device".into()));
                None
            }
        }
        None => None,
    };
    let mut output = OutputSettings::resolve(device_id, saved.mode, true);
    output.missing_device = missing;
    output
}

struct AppState {
    player: Player,
    output: Mutex<OutputSettings>,
    output_path: PathBuf,
    /// Follows the selected device's Windows volume, so the UI moves when it
    /// is changed in Windows, by volume keys or on the DAC.
    volume_watch: Mutex<Option<VolumeWatch>>,
    /// Bumped to call off a wait for a device that went away (see
    /// `reconnect_when_back`).
    reconnect: Mutex<u64>,
    playlists: Mutex<PlaylistStore>,
    eq: Mutex<EqSettings>,
    eq_path: PathBuf,
}

type CommandResult<T> = Result<T, String>;

fn text<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---- Output ----------------------------------------------------------------

#[tauri::command]
fn list_devices() -> CommandResult<Vec<DeviceInfo>> {
    mdm_engine::list_devices().map_err(text)
}

#[tauri::command]
fn get_output(state: State<AppState>) -> OutputSettings {
    state.output.lock().unwrap().clone()
}

/// Choose the output. Exclusive mode is only used on a device whose volume
/// works in it; otherwise shared mode is used, and the returned settings say so.
/// Switching to a device at full volume turns it down first; changing only
/// the mode leaves the level alone.
#[tauri::command(async)]
fn set_output(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: Option<String>,
    mode: OutputMode,
) -> OutputSettings {
    cancel_reconnect(&state);
    let device_changed = state.output.lock().unwrap().device_id != device_id;
    let output = OutputSettings::resolve(device_id, mode, device_changed);
    state.player.send(Command::SetOutput(Box::new(output.backend())));
    watch_volume(&app, &state, output.device_id.as_deref());
    *state.output.lock().unwrap() = output.clone();

    let device_name = output.device_id.as_ref().and_then(|id| {
        mdm_engine::list_devices().ok()?.into_iter().find(|d| &d.id == id).map(|d| d.name)
    });
    let saved = SavedOutput { device_id: output.device_id.clone(), device_name, mode: output.wanted };
    save_output(&state.output_path, &saved);
    output
}

// ---- Reconnecting ----------------------------------------------------------

/// How often to look for a device that went away.
const RECONNECT_POLL: Duration = Duration::from_secs(1);
/// A device still listed this long after it failed was not unplugged
/// (driver error, taken by another app): stop waiting for it.
const RECONNECT_GIVE_UP: Duration = Duration::from_secs(4);
/// Time for a replugged device's driver to settle before it is opened.
const RECONNECT_SETTLE: Duration = Duration::from_millis(700);

#[derive(Clone, Serialize)]
struct Reconnected {
    output: OutputSettings,
    /// The device that came back.
    device: String,
}

/// Stop waiting for a device that went away: the user took over.
fn cancel_reconnect(state: &AppState) {
    *state.reconnect.lock().unwrap() += 1;
}

/// The device playing `device_name` stopped mid-stream. Wait for it to come
/// back and resume there.
fn start_reconnect(app: &AppHandle, device_name: String) {
    let state = app.state::<AppState>();
    let generation = {
        let mut g = state.reconnect.lock().unwrap();
        *g += 1;
        *g
    };
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("mdm-reconnect".into())
        .spawn(move || reconnect_when_back(app, generation, device_name));
    if let Err(e) = spawned {
        tracing::warn!("not waiting for the device to come back: {e}");
    }
}

/// Poll the device list. Once the device has been gone and is back (and, when
/// following the Windows default, is the default again), reopen it with the
/// full-volume guard, since Windows tends to reset a replugged DAC to 100%,
/// and play on from where it stopped. A device that never leaves the list was
/// not unplugged, and is left alone.
fn reconnect_when_back(app: AppHandle, generation: u64, device_name: String) {
    let state = app.state::<AppState>();
    let current = || *state.reconnect.lock().unwrap() == generation;
    let started = Instant::now();
    let mut gone = false;
    loop {
        std::thread::sleep(RECONNECT_POLL);
        if !current() {
            return;
        }
        let output = state.output.lock().unwrap().clone();
        let devices = mdm_engine::list_devices().unwrap_or_default();
        let back = devices.iter().any(|d| match &output.device_id {
            Some(id) => &d.id == id,
            None => d.is_default && d.name == device_name,
        });
        if !back {
            if !gone {
                gone = true;
                let _ = app.emit("output://waiting", &device_name);
            }
            continue;
        }
        if !gone {
            if started.elapsed() >= RECONNECT_GIVE_UP {
                return;
            }
            continue;
        }
        std::thread::sleep(RECONNECT_SETTLE);
        if !current() {
            return;
        }
        let output = OutputSettings::resolve(output.device_id, output.wanted, true);
        state.player.send(Command::SetOutput(Box::new(output.backend())));
        watch_volume(&app, &state, output.device_id.as_deref());
        *state.output.lock().unwrap() = output.clone();
        state.player.send(Command::Play);
        let _ = app.emit("output://reconnected", Reconnected { output, device: device_name });
        return;
    }
}

/// Emit `volume://changed` whenever `device_id`'s volume changes, replacing
/// the watch on the previous device.
fn watch_volume(app: &AppHandle, state: &AppState, device_id: Option<&str>) {
    let mut watch = state.volume_watch.lock().unwrap();
    // Unregister from the old device before listening to the new one.
    *watch = None;
    let handle = app.clone();
    let emit = move |volume: DeviceVolume| {
        if let Err(e) = handle.emit("volume://changed", volume) {
            tracing::warn!("could not emit a volume event: {e}");
        }
    };
    match mdm_engine::watch_volume(device_id, emit) {
        Ok(w) => *watch = Some(w),
        // The UI still re-reads the volume on focus.
        Err(e) => tracing::warn!("not following the device volume: {e}"),
    }
}

/// The Windows volume of the selected output device.
#[tauri::command(async)]
fn get_volume(state: State<'_, AppState>) -> CommandResult<DeviceVolume> {
    let device_id = state.output.lock().unwrap().device_id.clone();
    mdm_engine::get_volume(device_id.as_deref()).map_err(text)
}

/// Change the Windows volume of the selected output device. This sets the
/// device's own level; the samples sent to it stay unchanged.
#[tauri::command(async)]
fn set_volume(
    state: State<'_, AppState>,
    level: Option<f32>,
    muted: Option<bool>,
) -> CommandResult<DeviceVolume> {
    let device_id = state.output.lock().unwrap().device_id.clone();
    mdm_engine::set_volume(device_id.as_deref(), VolumeChange { level, muted }).map_err(text)
}

// ---- Equalizer -------------------------------------------------------------

#[tauri::command]
fn get_eq(state: State<AppState>) -> EqSettings {
    state.eq.lock().unwrap().clone()
}

/// Change the equalizer and remember it. Returns the settings as applied,
/// after out-of-range values were brought into range.
#[tauri::command]
fn set_eq(state: State<AppState>, settings: EqSettings) -> CommandResult<EqSettings> {
    let settings = settings.sanitized();
    *state.eq.lock().unwrap() = settings.clone();
    state.player.send(Command::SetEq(settings.clone()));
    let json = serde_json::to_vec_pretty(&settings).map_err(text)?;
    if let Some(dir) = state.eq_path.parent() {
        std::fs::create_dir_all(dir).map_err(text)?;
    }
    std::fs::write(&state.eq_path, json).map_err(text)?;
    Ok(settings)
}

/// The saved equalizer, or the default (off, flat) when there is none.
fn load_eq(path: &Path) -> EqSettings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<EqSettings>(&bytes).ok())
        .map(EqSettings::sanitized)
        .unwrap_or_default()
}

// ---- Transport -------------------------------------------------------------

#[tauri::command]
fn supported_extensions() -> Vec<&'static str> {
    mdm_decode::EXTENSIONS.to_vec()
}

#[tauri::command]
fn play_files(state: State<AppState>, paths: Vec<String>, index: usize) {
    cancel_reconnect(&state);
    let paths = paths.into_iter().map(PathBuf::from).collect();
    state.player.send(Command::Load { paths, index });
}

/// Add tracks to the end of the queue without interrupting playback.
#[tauri::command]
fn append_files(state: State<AppState>, paths: Vec<String>) {
    state.player.send(Command::Append(paths.into_iter().map(PathBuf::from).collect()));
}

/// Take one track out of the queue. The UI removes it from its own list too.
#[tauri::command]
fn queue_remove(state: State<AppState>, index: usize) {
    state.player.send(Command::Remove(index));
}

/// Move a queue track so it ends up at `to`. The UI moves it in its own list too.
#[tauri::command]
fn queue_move(state: State<AppState>, from: usize, to: usize) {
    state.player.send(Command::Move { from, to });
}

#[tauri::command]
fn set_shuffle(state: State<AppState>, on: bool) {
    state.player.send(Command::SetShuffle(on));
}

#[tauri::command]
fn set_repeat(state: State<AppState>, repeat: Repeat) {
    state.player.send(Command::SetRepeat(repeat));
}

#[tauri::command]
fn play(state: State<AppState>) {
    cancel_reconnect(&state);
    state.player.send(Command::Play);
}

#[tauri::command]
fn pause(state: State<AppState>) {
    state.player.send(Command::Pause);
}

#[tauri::command]
fn stop(state: State<AppState>) {
    cancel_reconnect(&state);
    state.player.send(Command::Stop);
}

#[tauri::command]
fn seek(state: State<AppState>, ms: u64) {
    state.player.send(Command::Seek(ms));
}

#[tauri::command]
fn next(state: State<AppState>) {
    state.player.send(Command::Next);
}

#[tauri::command]
fn previous(state: State<AppState>) {
    state.player.send(Command::Previous);
}

// ---- Library ---------------------------------------------------------------
// Commands marked `async` read files, so they run off the main thread.

/// Turn dropped or picked files and folders into tracks with their tags.
#[tauri::command(async)]
fn expand_paths(paths: Vec<String>) -> Vec<TrackMeta> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    mdm_library::expand_paths(&paths)
}

/// Everything known about one file, for the track info panel.
#[tauri::command(async)]
fn track_details(path: String) -> CommandResult<TrackDetails> {
    let path = Path::new(&path);
    if !path.is_file() {
        return Err("The file no longer exists.".into());
    }
    Ok(mdm_library::read_details(path))
}

#[tauri::command(async)]
fn get_lyrics(path: String) -> Option<Lyrics> {
    mdm_library::read_lyrics(Path::new(&path))
}

#[tauri::command]
fn playlists(state: State<AppState>) -> Vec<Playlist> {
    state.playlists.lock().unwrap().list().to_vec()
}

#[tauri::command]
fn playlist_create(state: State<AppState>, name: String) -> CommandResult<Playlist> {
    state.playlists.lock().unwrap().create(&name).map_err(text)
}

#[tauri::command]
fn playlist_rename(state: State<AppState>, id: u64, name: String) -> CommandResult<Playlist> {
    state.playlists.lock().unwrap().rename(id, &name).map_err(text)
}

#[tauri::command]
fn playlist_delete(state: State<AppState>, id: u64) -> CommandResult<()> {
    state.playlists.lock().unwrap().delete(id).map_err(text)
}

#[derive(Serialize)]
struct PlaylistAdded {
    playlist: Playlist,
    /// Paths that were neither a folder nor a playable file.
    skipped: Vec<String>,
}

/// Add folders and audio files to a playlist.
#[tauri::command]
fn playlist_add(state: State<AppState>, id: u64, paths: Vec<String>) -> CommandResult<PlaylistAdded> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (playlist, skipped) = state.playlists.lock().unwrap().add_paths(id, &paths).map_err(text)?;
    Ok(PlaylistAdded { playlist, skipped })
}

#[tauri::command]
fn playlist_remove_file(state: State<AppState>, id: u64, file: String) -> CommandResult<Playlist> {
    state.playlists.lock().unwrap().remove_file(id, &file).map_err(text)
}

/// Open File Explorer with the track selected.
#[tauri::command]
fn show_in_folder(path: String) -> CommandResult<()> {
    let path = Path::new(&path);
    if !path.is_file() {
        return Err("The file no longer exists.".into());
    }
    // Explorer wants backslashes, and takes "/select," and the path as one argument.
    let path: PathBuf = path.components().collect();
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(text)
}

#[tauri::command]
fn playlist_remove_folder(state: State<AppState>, id: u64, folder: String) -> CommandResult<Playlist> {
    state.playlists.lock().unwrap().remove_folder(id, &folder).map_err(text)
}

/// Read the playlist's folders from disk and return its tracks.
#[tauri::command(async)]
fn playlist_scan(state: State<AppState>, id: u64) -> CommandResult<Vec<TrackMeta>> {
    // Copy the playlist out so the lock is not held during the scan.
    let playlist = state.playlists.lock().unwrap().get(id).map_err(text)?.clone();
    Ok(playlist.scan())
}

// ---- Cover art -------------------------------------------------------------

/// Answer `cover://<track path>` with the track's cover image.
fn cover_response(uri_path: &str) -> Response<Vec<u8>> {
    let not_found = || {
        Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()).expect("static response")
    };
    let decoded = percent_encoding::percent_decode_str(uri_path.trim_start_matches('/'))
        .decode_utf8_lossy()
        .into_owned();
    let track = Path::new(&decoded);
    // Only ever read pictures that belong to audio files.
    if !mdm_library::is_audio_file(track) {
        return not_found();
    }
    match mdm_library::read_cover(track) {
        Some(cover) => Response::builder()
            .header(header::CONTENT_TYPE, cover.mime)
            .header(header::CACHE_CONTROL, "max-age=600")
            .body(cover.data)
            .unwrap_or_else(|_| not_found()),
        None => not_found(),
    }
}

// ---- Startup ---------------------------------------------------------------

fn event_name(event: &Event) -> &'static str {
    match event {
        Event::State { .. } => "player://state",
        Event::Track { .. } => "player://track",
        Event::Current { .. } => "player://current",
        Event::Position { .. } => "player://position",
        Event::Error { .. } => "player://error",
        Event::DeviceLost { .. } => "player://device-lost",
        Event::QueueEnded => "player://queue-ended",
    }
}

/// A media key or the Windows media overlay was used.
fn on_media_button(app: &AppHandle, button: Button) {
    let state = app.state::<AppState>();
    let command = match button {
        Button::Play => {
            cancel_reconnect(&state);
            Command::Play
        }
        Button::Pause => Command::Pause,
        Button::Stop => {
            cancel_reconnect(&state);
            Command::Stop
        }
        Button::Next => Command::Next,
        Button::Previous => Command::Previous,
    };
    state.player.send(command);
}

/// How long closing the app waits for the fade-out and the device to be let go.
const EXIT_WAIT: Duration = Duration::from_millis(1500);

/// Open the playlist file. If it cannot be read, set it aside and start
/// empty rather than refusing to launch.
fn open_playlists(path: &Path) -> PlaylistStore {
    match PlaylistStore::open(path) {
        Ok(store) => store,
        Err(e) => {
            tracing::error!("playlists could not be loaded ({e}); starting with none");
            let _ = std::fs::rename(path, path.with_extension("json.damaged"));
            PlaylistStore::open(path).expect("an absent playlist file opens as empty")
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("cover", |_ctx, request, responder| {
            let uri_path = request.uri().path().to_string();
            std::thread::spawn(move || responder.respond(cover_response(&uri_path)));
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let output_path = data_dir.join("output.json");
            let output = startup_output(&load_saved_output(&output_path));
            let player = Player::new(Box::new(output.backend()));
            let events = player.events();
            let playlists = open_playlists(&data_dir.join("playlists.json"));
            let eq_path = data_dir.join("eq.json");
            let eq = load_eq(&eq_path);
            player.send(Command::SetEq(eq.clone()));
            let device_id = output.device_id.clone();
            app.manage(AppState {
                player,
                output: Mutex::new(output),
                output_path,
                volume_watch: Mutex::new(None),
                reconnect: Mutex::new(0),
                playlists: Mutex::new(playlists),
                eq: Mutex::new(eq),
                eq_path,
            });
            watch_volume(app.handle(), &app.state::<AppState>(), device_id.as_deref());

            // Media keys and the Windows media overlay. The app works without them.
            let media = app.get_webview_window("main").and_then(|window| {
                let hwnd = window.hwnd().ok()?;
                let handle = app.handle().clone();
                MediaControls::new(hwnd, move |button| on_media_button(&handle, button))
                    .map_err(|e| tracing::warn!("no media controls: {e}"))
                    .ok()
            });

            // Forward player events to the UI for as long as the player lives.
            let handle = app.handle().clone();
            std::thread::Builder::new().name("mdm-events".into()).spawn(move || {
                // The device of the track being heard, to wait for if it goes away.
                let mut device = String::new();
                for event in events {
                    match &event {
                        Event::State { state } => {
                            if let Some(m) = &media {
                                m.set_state(*state);
                            }
                        }
                        Event::Track { path, signal_path, .. } => {
                            device = signal_path.device.clone();
                            if let Some(m) = &media {
                                m.set_track(path);
                            }
                        }
                        Event::DeviceLost { .. } => start_reconnect(&handle, device.clone()),
                        _ => {}
                    }
                    if let Err(e) = handle.emit(event_name(&event), &event) {
                        tracing::warn!("could not emit a player event: {e}");
                    }
                }
            })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            get_output,
            set_output,
            get_volume,
            set_volume,
            get_eq,
            set_eq,
            supported_extensions,
            play_files,
            append_files,
            queue_remove,
            queue_move,
            set_shuffle,
            set_repeat,
            play,
            pause,
            stop,
            seek,
            next,
            previous,
            expand_paths,
            get_lyrics,
            track_details,
            playlists,
            playlist_create,
            playlist_rename,
            playlist_delete,
            playlist_add,
            playlist_remove_file,
            show_in_folder,
            playlist_remove_folder,
            playlist_scan,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start MDMusic")
        .run(|app, event| {
            // The window is gone: fade out and release the audio device
            // before the process goes away.
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                cancel_reconnect(&state);
                state.player.quit(EXIT_WAIT);
                state.volume_watch.lock().unwrap().take();
            }
        });
}
