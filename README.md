# MDMusic

A music player for Windows that sends audio to a USB DAC unchanged (bit-perfect).
The core is Rust; the window is Tauri 2 with a Svelte 5 UI.

## Run it

You need Rust (stable), Node.js 20 or newer, and the WebView2 runtime (part of Windows 11).

```sh
npm install
npm run dev        # the desktop app, with hot reload
npm run build      # a release build and an installer
cargo test         # decoder and engine tests, no audio hardware needed
```

The engine also runs on its own from the command line:

```sh
cargo run -p mdm-engine --example play -- --list
cargo run -p mdm-engine --example play -- --device "DX1" album/*.flac
```

## How playback works

```
file ─► mdm-decode ─► pack / DoP ─► lock-free ring ─► render thread ─► DAC
        (player thread)                              (WASAPI exclusive)
```

- **Exclusive mode** opens the device at the file's own sample rate and takes it
  over, so the Windows mixer never touches the audio. Other apps are silent on
  that device while MDMusic plays.
- **Samples are only shifted** into a container the DAC accepts (16-bit, 24-bit
  packed, or 32-bit). Nothing scales, dithers or resamples them. If the DAC
  cannot play a file's rate or bit depth, that track is skipped with an error
  instead of being converted.
- **DSD** (DSF and DFF files) goes out as DoP: 16 DSD bits per 24-bit PCM
  sample under an alternating marker byte. The DAC must support DoP.
- **Tracks with the same format play gapless.** A format change reopens the
  device, which gives a short gap.
- **Shared mode** is there for when other apps must keep playing. Windows then
  resamples and mixes, and the UI says so.

## Layout

| Path | What it is |
|---|---|
| `crates/mdm-decode` | File decoding: PCM through symphonia, raw DSD from DSF/DFF |
| `crates/mdm-engine` | Player, format negotiation, DoP, WASAPI and file outputs |
| `crates/mdm-library` | Tags, cover art, lyrics, folder scanning and playlists |
| `src-tauri` | The desktop app: Tauri commands and events over the engine |
| `ui` | The Svelte UI |

## Playlists, covers and lyrics

- **A playlist holds folders and single tracks.** Every audio file inside its
  folders (subfolders included) is in the playlist, in album order, followed
  by the tracks added one by one. Add with the Add folder and Add files
  buttons, by dropping folders or files onto the playlist, or from the track
  menu. Press Rescan after files change on disk. Deleting a playlist never
  deletes music.
- **Right-click a track** for its menu: play it, add it to the queue, add it
  to a playlist (or to a new one), remove it from the playlist, or show it in
  File Explorer. Tracks that are in a playlist because of a folder can only be
  removed by removing the folder.
- **Shuffle** (the button left of Previous) plays the queue in random order,
  starting from the track you picked. Every track plays once before the queue
  ends, and tracks of the same format still play gapless.
- **Cover art** comes from the picture embedded in the file. Without one, an
  image named `cover`, `folder`, `front` or `album` (jpg, png or webp) in the
  same folder is used.
- **Lyrics** come from a `.lrc` file with the same name as the track, or else
  from the lyrics tag. Timed lyrics follow the music, and clicking a line
  jumps to it.

Playlists are stored in `%APPDATA%\com.mdmusic.app\playlists.json`.

## Status

Working: playback of a queue of files and folders, pause, seek, next and
previous, shuffle, gapless, device and mode selection, DSD over DoP, a
signal-path badge, playlists of folders and tracks, a track menu, tags, cover
art and lyrics.

Not built yet: search and a full library index, reordering playlists and the
queue, repeat, tags for DSF/DFF files (they show file names), media keys and tray,
automatic recovery when a DAC is replugged (playback stops with an error; press
play to resume), volume control, and an ASIO output for native DSD.
