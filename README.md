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
- **Pause, resume and closing the app fade** the sound out and in. The fade
  moves the device's own Windows volume, not the samples, so playback stays
  bit-perfect and DSD fades too. After a pause the level goes back to where you
  set it, behind the silence. In shared mode a fade also dips other apps on the
  same device for a moment.
- **The output device is remembered.** If it is not connected at launch, the
  system default is used for that run.
- **Unplug the DAC mid-track** and playback stops; plug it back in and it
  carries on from the same spot. A DAC that comes back at 100% is turned down
  to 10% first.
- **Media keys** and the Windows media overlay (next to the volume flyout and
  on the lock screen) show the track and its cover, and control playback.

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
- **Sort a playlist** by clicking a column heading (Title, Artist, Album or
  Time): once for A to Z, again for Z to A, a third time (or the # heading)
  for the playlist's own order. Play follows the order on screen, and each
  playlist remembers its sort.
- **Right-click a track** for its menu: play it, add it to the queue, add it
  to a playlist (or to a new one), remove it from the playlist, or show it in
  File Explorer. Tracks that are in a playlist because of a folder can only be
  removed by removing the folder.
- **Shuffle** (the button left of Previous) plays the queue in random order,
  starting from the track you picked. Every track plays once before the queue
  ends, and tracks of the same format still play gapless.
- **Repeat** (the button right of Next) goes off, all, one. Repeat all starts
  the queue over after the last track; repeat one plays the current track again,
  gapless. Next and Previous still move between tracks.
- **Edit the queue**: drag tracks to move them, or right-click and choose
  Remove from queue (or press Delete). Playback carries on.
- **Select several tracks** in any list: Ctrl+click adds one, Shift+click a
  range, Ctrl+A all. The right-click menu and Delete then act on all of them.
- **Search** (the box above Queue, or Ctrl+F) looks through every playlist by
  title, artist, album, file or folder name. Case and accents do not matter:
  "hiep si" finds "Hiệp sĩ".
- **Drag playlists** in the sidebar to put them in the order you like.
- **Cover art** comes from the picture embedded in the file. Without one, an
  image named `cover`, `folder`, `front` or `album` (jpg, png or webp) in the
  same folder is used. Track lists show a small copy on every row. Click the
  cover to turn it into a round record that spins while the music plays; click
  again for the square cover.
- **DSF and DFF files** show their tags, cover and lyrics like any other file
  (from their ID3 tag, or a DFF's own title and artist).
- **Lyrics** come from a `.lrc` file with the same name as the track, or else
  from the lyrics tag. Timed lyrics follow the music, and clicking a line
  jumps to it.

Playlists are stored in `%APPDATA%\com.mdmusic.app\playlists.json`, the
equalizer in `eq.json` and the output device in `output.json` next to it.
`library.db` there is an index of every playlist's tracks (tags and small
covers), so rescans are quick; it is rebuilt from your files if deleted.

## Status

Working: playback of a queue of files and folders, pause and resume with
fades, seek, next and previous, shuffle, repeat (all or one), gapless, moving
and removing tracks in the queue, selecting several tracks, search across all
playlists, device and mode selection (remembered), resuming after the DAC is
replugged, media keys and the Windows media overlay, the device's volume
(bit-perfect), an opt-in equalizer, DSD over DoP with tags, a signal-path
badge, playlists of folders and tracks (in your own order), sorting a playlist
by column, a track menu, tags, cover art (square, as a spinning record, and
small in every list) and lyrics.

Not built yet: browsing by artist or album, editing tags, a tray icon, and an
ASIO output for native DSD.
