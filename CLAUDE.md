# MDMusic

A Windows music player that sends audio to a USB DAC unchanged (bit-perfect).
Rust core, Tauri 2 shell, Svelte 5 + Tailwind 4 UI. See README.md for the user-facing overview.

## Commands

```sh
npm run dev                      # the desktop app with hot reload
npm run check                    # svelte-check (types) for the UI
cargo test                       # decode, engine and library tests; no audio hardware needed
cargo clippy --workspace --all-targets
cargo build -p mdmusic           # the Tauri app; not in default-members, so plain `cargo test` skips it
cargo run -p mdm-engine --example play -- --list          # command-line player
```

Run all four checks (test, clippy, `npm run check`, `cargo build -p mdmusic`) before calling a change done.

## Layout

| Path | Role |
|---|---|
| `crates/mdm-decode` | Files to samples. PCM via symphonia, raw DSD from DSF/DFF. |
| `crates/mdm-engine` | Player thread, format negotiation, DoP, outputs (`output/wasapi.rs`, `output/file_sink.rs`). |
| `crates/mdm-library` | Tags (`tagged.rs` opens every file, DSD too), cover art, lyrics (`.lrc` and embedded), folder scan, playlists, the SQLite library index (`index.rs`). |
| `src-tauri` | Tauri commands and events over the two crates above. `capabilities/default.json` lists what the UI may call. |
| `ui` | Svelte UI. `src/lib/api.ts` is the only place that calls `invoke`. |

Adding a feature that crosses the boundary means touching, in order: the crate,
a `#[tauri::command]` in `src-tauri/src/lib.rs` (and the `generate_handler!` list),
a typed wrapper in `ui/src/lib/api.ts`, then the component.

## Rules that must hold

- **Bit-perfect path.** Between decode and device, samples are only shifted into
  a container (`format::pack`). Never scale, dither, resample or mix on the
  exclusive path. A feature that changes samples (volume, EQ, ReplayGain) must
  be opt-in and must clear `SignalPath::bit_perfect`.
- **PCM is left-justified `i32`** everywhere after decode: a 16-bit sample sits
  in the top 16 bits. Containers are cut from the top bytes.
- **Render thread is real-time.** `Feeder::fill` and the loop in
  `output/wasapi.rs::render` must not allocate, lock or log per buffer.
- **DoP markers are written at the device end** (`MarkerStamper`), not by the
  packer, so silence and audio share one unbroken marker sequence.
- **The equalizer (`eq.rs`) is the one opt-in sample change.** It is off by
  default, runs on the player thread before `pack` (never in the render
  loop), sets `SignalPath::eq` and clears `bit_perfect`. With it on, PCM asks
  for at least 24 bits and `exact: false`; DSD is never equalized. Toggling
  it restarts the current track at the same position. Settings are saved in
  `%APPDATA%\com.mdmusic.app\eq.json`.
- **A track the DAC cannot play is skipped with an error**, not converted.
- **The volume bar is the device's Windows volume** (`output/volume.rs`,
  `IAudioEndpointVolume`), never sample scaling, so it keeps bit-perfect.
  It only works in exclusive mode when the device reports an adjustable
  hardware volume (`DeviceVolume::hardware`; a range fixed at one level counts
  as none). Without one, `set_output` (`OutputSettings::resolve`) uses shared
  mode instead and the UI disables Exclusive, unless the software volume is on
  (next rule). The user's pick (`wanted`) is
  kept, so exclusive comes back on a device that has a hardware volume.
  Changes made outside the app (Windows mixer, volume keys, the DAC) reach the
  UI through `watch_volume` (`IAudioEndpointVolumeCallback`) as the
  `volume://changed` event; `set_output` moves the watch to the new device.
  A device found at 100% at launch or when switched to is turned down to 10%
  before its stream opens (`FULL_LEVEL`/`SAFE_LEVEL` in `src-tauri/src/lib.rs`),
  since Windows often resets USB DACs to 100%. Changing only the mode does not.
  The chosen device and mode are saved in `%APPDATA%\com.mdmusic.app\output.json`
  (only by `set_output`). A saved device that is not connected at launch falls
  back to the Windows default for that run (`missing_device`), keeping the file.
- **The software volume is the second opt-in sample change**, only for a
  device with no hardware volume (`output/soft_volume.rs`). It is used when
  the user turned it on (`set_soft_volume`, saved as `soft_volume` in
  `output.json`), the device has no hardware volume, and the mode is
  exclusive: then `OutputSettings::soft_volume` is set, the backend carries
  the shared `SoftVolume`, and the volume bar (`get_volume`/`set_volume`,
  `DeviceVolume::software`) drives it instead of Windows. The level is kept
  per device in `soft_levels` and starts at `SAFE_LEVEL`; the slider maps to
  gain by a cube law (`taper`). It scales in `Feeder` (the device end, so a
  change is heard at once), sliding across each buffer, rounding to the
  stream's valid bits. Like the EQ, PCM then asks for at least 24 bits and
  `exact: false`, and `SignalPath::bit_perfect` is cleared
  (`SignalPath::soft_volume`). **DSD is refused with an error** while it is in
  use, since DoP cannot be scaled and would play at full level. Fades go
  through `SoftFader`, a factor on top of the user's level.
- **Fades move the device volume, never the samples.** Pause fades out
  (`FADE_OUT`), resume fades in, closing the app fades out (`Player::quit`).
  The engine drives a `Fader` from `Backend::fader()`; for WASAPI that is
  `EndpointFader` (`output/volume.rs`), which sets the Windows volume with the
  `FADE_CONTEXT` GUID so `watch_volume` ignores those steps and the slider
  stays put. After a fade-out the level goes back up behind the silence
  (`FADE_SETTLE`), so a pause never leaves the device turned down. In shared
  mode a fade briefly dips other apps on the same device too.
- **A device unplugged mid-track is waited for** (`reconnect_when_back` in
  `src-tauri/src/lib.rs`): the engine stops with `Event::DeviceLost`, and once
  the device has left the device list and come back (as the default again when
  following the default) it is reopened through `OutputSettings::resolve` with
  the full-volume guard and plays on. A device that never leaves the list is
  not waited for. Any user choice (play, stop, new queue, new output) calls the
  wait off (`cancel_reconnect`).
- **Media keys** go through the Windows media controls
  (`src-tauri/src/media_controls.rs`, SMTC on the main window). Their buttons
  send engine commands directly; the UI follows from the engine's events.
- **A playlist is folders plus single files.** Its tracks are the folders'
  audio (scanned each time, album order), then the single files in the order
  added, with duplicates dropped. A track that comes from a folder cannot be
  removed on its own; only single files can. Deleting a playlist never touches
  music files. `files` is `#[serde(default)]` so older playlist files still load.
- **Sorting a playlist is a view, done in the UI** (`ui/src/lib/sort.ts`). It
  never changes `playlists.json` or what `scan()` returns. `playlistTracks` in
  `App.svelte` is the sorted list, and the table, the track menu's row index
  and `playTracks` must all use that same list. The sort of each playlist is
  kept in localStorage (`playlistSort`). The queue is not sortable: its order
  belongs to the engine.
- **The play order lives in the engine** (`Engine::order`). Shuffle, repeat,
  Next, Previous, skipping bad tracks and gapless line-up all go through
  `following()` / `preceding()` (which apply repeat on top of `after()` /
  `before()`); never step through the queue with `index + 1`. Skipping loops
  are bounded by the queue length, since with repeat on the order never ends.
- **The UI's queue and the engine's queue must match.** Replace with
  `play_files`, extend with `append_files`, put tracks next with `queue_insert`
  (at the current index + 1; with shuffle on the engine also makes them next
  in `order`), edit with `queue_remove` (a set of
  indexes) / `queue_reorder` (`order[new] = old`, built with `moveRows` in
  `ui/src/lib/reorder.svelte.ts`). The UI changes its own `queue` and
  `currentIndex` the same way; the engine confirms with `player://current`. Changing `queue` in the UI
  alone leaves the player playing the old list. After any edit the engine runs
  `fix_line_up()`: audio already in the ring for a track that should no longer
  come next is dropped by restarting the current track where it is.
- **Tag reading never fails.** `read_meta` falls back to the file name. Open
  tags with `tagged::read_tagged`, never `lofty::read_from_path`: lofty does
  not know DSF/DFF, so their ID3v2 block (DSF: pointed to by the header; DFF:
  the `ID3 ` chunk, else `DIIN` title/artist) is handed to lofty inside a
  minimal in-memory WAV. DSD durations come from the decoder.
- **The library index (`library.db`) is a cache, never the source.** It holds
  exactly the tracks of the playlists: `Library::refresh` (run by the
  `mdm-library` thread at launch and after every playlist change, see
  `refresh_library()`) scans them all and drops other rows. A row is current
  while the file's size and modification time match. `expand_paths` (the
  queue) reads through the cache but never writes. Bump `SCHEMA_VERSION` when
  the tables change; an old or damaged database is simply rebuilt.
- **Search is folded text, not FTS**: `fold()` lowercases and strips accents
  (and maps đ to d), and every query word must appear (`LIKE`, wildcards
  escaped) in title, artist, album, album artist, file or folder name.
- **Small covers** are `cover://<path>?thumb`: 96 px JPEGs made by
  `Library::thumbnail`, kept in the `thumbs` table, at most `THUMB_WORKERS`
  made at once. Rows load them lazily (`Thumb.svelte`).
- **Selection is by track object** (`Selection` in
  `ui/src/lib/selection.svelte.ts`), so it survives moves and sorting. Track
  lists report rows (`onmenu(rows)`, `ondelete(rows)`, `onmove(rows, slot)`),
  always in list order. Dragging uses `RowDrag` (pointer events); a row's
  click handler must first check `drag.endedDrag()`, or the release of a drag
  counts as a click.

## Testing

- `crates/mdm-engine/tests/bit_perfect.rs` plays files through the whole player
  into `FileSink` and compares bytes with the source. Any engine change must
  keep these passing; add a case there for new formats or paths.
- Tests build their own WAV/DSF/DFF/tagged files in the temp folder. Do not
  commit audio fixtures.
- For tests that act while a track plays (pause, fades, queue edits), set
  `FileSink::paced` so it takes audio at the real rate, and `fader_log` to
  record the fader's levels (`capture` is logged as `-1.0`).
- **Never play audible audio on this machine to test.** Use silent files
  (all-zero samples); they exercise the full WASAPI path with no sound.
- Devices here: "Speakers (DX1 II)" is the USB DAC (accepts only 32-bit words
  with 24 valid bits; 44.1–96 kHz verified; it is now the Windows default).
  "1 - GIGABYTE G24F" (HDMI) and "Digital Audio (S/PDIF)" have no hardware
  volume, so they test the software volume and the shared fallback. Switching
  to a device sitting at 100% turns it down to 10% (the full-volume guard):
  put its Windows volume back after a test.
- To drive the real UI, start the app with
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and talk
  Chrome DevTools Protocol to the page at `localhost:1420`. Native dialogs
  (file and folder pickers) cannot be driven; call the command behind them.
  Clean up any playlists a test creates: they live in
  `%APPDATA%\com.mdmusic.app\playlists.json`, the user's real data. Picking a
  device writes `output.json` there too, and `library.db` is rebuilt from the
  playlists, so a test playlist's tracks leave it on the next refresh.
- A real click with Ctrl or Shift needs `modifiers` on every
  `Input.dispatchMouseEvent` (2 = Ctrl, 8 = Shift). A synthetic click event
  followed by a real release gives a second, plain click.
- Synthetic events need `bubbles: true`: Svelte 5 delegates `onchange`,
  `onclick` and the like to the root. Queue rows are dragged with real input
  (`Input.dispatchMouseEvent`), since they use pointer events. A file drop can
  be faked with `invoke("plugin:event|emit", { event: "tauri://drag-drop",
  payload: { paths, position } })`.
- The media controls can be read and pressed from outside, as Windows does:
  `GlobalSystemMediaTransportControlsSessionManager` (WinRT, usable from
  PowerShell) lists MDMusic as `mdmusic.exe`.

## Gotchas

- Tailwind theme colours are named `base`, `panel`, `raised`, `line`, `ink`,
  `muted`, `accent`, `good`, `warn`, `bad` (`ui/src/app.css`). So **`text-base`
  is the dark background colour here, not a font size**. Use `text-[15px]` or
  `text-sm`/`text-lg` for size; `text-base` is only for dark text on a light button.
- `wasapi` 0.27 and `symphonia` 0.6 differ from older examples online. Read the
  crate source under `~/.cargo/registry/src` instead of guessing.
- In lofty, ID3v2 lyrics (USLT) are `ItemKey::UnsyncLyrics`; Vorbis and MP4
  lyrics are `ItemKey::Lyrics`. Read both.
- The browser's own right-click menu is turned off app-wide (`onContextMenu`
  in `App.svelte`), except in text fields and over selected text. Track rows
  open `ContextMenu.svelte`; its items are built in `trackMenu()`.
- `ContextMenu` submenus are `position: fixed` inside the parent menu. Never
  give the menu a `transform`, `filter` or `backdrop-filter` (Tailwind
  `backdrop-blur-*`, `scale-*`, ...): that makes it the containing block and
  the submenu lands in the wrong place.
- Cover art reaches the UI through the `cover://` scheme (`coverUrl()` in
  `api.ts`), served by `cover_response` in `src-tauri/src/lib.rs`. A new image
  source needs its host added to `img-src` in `tauri.conf.json`.
- `Cover.svelte` in disc mode turns only the inner `.picture`; the outer
  `.frame` holds the round clip and the shadow and must stay still, or the
  shadow circles the cover. The mode is saved in localStorage (`coverDisc`).
- Serde names: Rust struct fields stay snake_case in JSON (`duration_ms`);
  command arguments are camelCase on the JS side (`deviceId`).
- Tauri is pinned to v2. v3 exists only as an alpha.
- Uses npm workspaces, not pnpm.
- In this environment the Bash tool breaks on heredocs containing backticks or
  odd quotes. Write files with the Write tool instead.
- A terminal opened before Rust was installed has no `cargo` on PATH. Prefix
  with `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"` or restart VS Code.

## Not built yet

Reordering tracks inside a playlist (only its folders' and files' own order,
which sorting covers for now), browsing the library by artist or album,
editing tags, a tray icon, a timeline (seek bar) in the Windows media overlay,
a setting to turn the fades off or change their length. ASIO output for native
DSD was set aside: the DX1 II runs on Windows' class driver (no ASIO), the
only ASIO driver here is Realtek's (no DSD), and Steinberg's SDK is
proprietary or GPLv3 while MDMusic is MIT. DSD stays on DoP until a
DSD-capable ASIO driver and an SDK license decision exist.
