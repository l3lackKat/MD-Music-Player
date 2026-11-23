//! Windows media controls (System Media Transport Controls): the keyboard's
//! media keys, the media overlay next to the volume flyout, and the lock
//! screen. They show the track and its cover and send Play, Pause, Next,
//! Previous and Stop to the player.
//!
//! The controls are tied to the main window. Updates go through a thread of
//! their own, so reading tags and covers never holds up player events.

use std::path::Path;
use std::sync::mpsc;

use mdm_engine::PlayState;
use windows::core::{HSTRING, Ref};
use windows::Foundation::TypedEventHandler;
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
    SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;

/// A media button pressed anywhere in Windows.
#[derive(Debug, Clone, Copy)]
pub enum Button {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

enum Update {
    State(PlayState),
    Track(String),
}

/// Sends updates to the controls. Dropping it takes MDMusic off them.
pub struct MediaControls {
    tx: mpsc::Sender<Update>,
}

impl MediaControls {
    /// Register `window` with the controls. `on_button` runs on a Windows
    /// thread: keep it short. `cover` gives a track's cover picture; it runs
    /// on the controls' own thread and may take its time.
    pub fn new(
        window: HWND,
        on_button: impl Fn(Button) + Send + 'static,
        cover: impl Fn(&Path) -> Option<Vec<u8>> + Send + 'static,
    ) -> windows::core::Result<Self> {
        let interop = windows::core::factory::<
            SystemMediaTransportControls,
            ISystemMediaTransportControlsInterop,
        >()?;
        let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(window)? };
        smtc.SetIsEnabled(true)?;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsStopEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        smtc.SetPlaybackStatus(MediaPlaybackStatus::Stopped)?;
        smtc.ButtonPressed(&TypedEventHandler::new(
            move |_: Ref<SystemMediaTransportControls>,
                  args: Ref<SystemMediaTransportControlsButtonPressedEventArgs>| {
                let button = match args.ok()?.Button()? {
                    SystemMediaTransportControlsButton::Play => Button::Play,
                    SystemMediaTransportControlsButton::Pause => Button::Pause,
                    SystemMediaTransportControlsButton::Stop => Button::Stop,
                    SystemMediaTransportControlsButton::Next => Button::Next,
                    SystemMediaTransportControlsButton::Previous => Button::Previous,
                    _ => return Ok(()),
                };
                on_button(button);
                Ok(())
            },
        ))?;

        let (tx, rx) = mpsc::channel::<Update>();
        std::thread::Builder::new()
            .name("mdm-media-controls".into())
            .spawn(move || {
                // Waiting on WinRT operations needs a multithreaded apartment.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                for update in rx {
                    let result = match update {
                        Update::State(state) => smtc.SetPlaybackStatus(match state {
                            PlayState::Playing => MediaPlaybackStatus::Playing,
                            PlayState::Paused => MediaPlaybackStatus::Paused,
                            PlayState::Stopped => MediaPlaybackStatus::Stopped,
                        }),
                        Update::Track(path) => show_track(&smtc, Path::new(&path), &cover),
                    };
                    if let Err(e) = result {
                        tracing::warn!("media controls: {e}");
                    }
                }
                let _ = smtc.DisplayUpdater().and_then(|u| u.ClearAll());
                let _ = smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed);
                let _ = smtc.SetIsEnabled(false);
            })
            .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
        Ok(MediaControls { tx })
    }

    pub fn set_state(&self, state: PlayState) {
        let _ = self.tx.send(Update::State(state));
    }

    /// Show this file's title, artist, album and cover.
    pub fn set_track(&self, path: &str) {
        let _ = self.tx.send(Update::Track(path.to_owned()));
    }
}

fn show_track(
    smtc: &SystemMediaTransportControls,
    path: &Path,
    cover: &dyn Fn(&Path) -> Option<Vec<u8>>,
) -> windows::core::Result<()> {
    let meta = mdm_library::read_meta(path);
    let updater = smtc.DisplayUpdater()?;
    updater.ClearAll()?;
    updater.SetType(MediaPlaybackType::Music)?;
    let music = updater.MusicProperties()?;
    music.SetTitle(&HSTRING::from(meta.title.as_str()))?;
    if let Some(artist) = &meta.artist {
        music.SetArtist(&HSTRING::from(artist.as_str()))?;
    }
    if let Some(album) = &meta.album {
        music.SetAlbumTitle(&HSTRING::from(album.as_str()))?;
    }
    if let Some(album_artist) = &meta.album_artist {
        music.SetAlbumArtist(&HSTRING::from(album_artist.as_str()))?;
    }
    // The words first: a cover from the internet can take a while.
    updater.Update()?;
    if let Some(cover) = cover(path) {
        // A bad picture still leaves the title and artist showing.
        match picture(&cover) {
            Ok(thumbnail) => {
                updater.SetThumbnail(&thumbnail)?;
                updater.Update()?;
            }
            Err(e) => tracing::warn!("media controls: cover not shown: {e}"),
        }
    }
    Ok(())
}

/// An in-memory image the controls can show.
fn picture(bytes: &[u8]) -> windows::core::Result<RandomAccessStreamReference> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    // Keep the stream open when the writer goes.
    writer.DetachStream()?;
    stream.Seek(0)?;
    RandomAccessStreamReference::CreateFromStream(&stream)
}
