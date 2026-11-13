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
| `crates/mdm-library` | Tags, cover art, lyrics (`.lrc` and embedded), folder scan, playlists. |
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
- **A track the DAC cannot play is skipped with an error**, not converted.
- **Playlists are folders, not track lists.** Tracks come from scanning the
  folders. Deleting a playlist never touches music files.
- **Tag reading never fails.** `read_meta` falls back to the file name. The tag
  reader (lofty) does not know DSF/DFF, so DSD files show file names only.

## Testing

- `crates/mdm-engine/tests/bit_perfect.rs` plays files through the whole player
  into `FileSink` and compares bytes with the source. Any engine change must
  keep these passing; add a case there for new formats or paths.
- Tests build their own WAV/DSF/DFF/tagged files in the temp folder. Do not
  commit audio fixtures.
- **Never play audible audio on this machine to test.** Use silent files
  (all-zero samples); they exercise the full WASAPI path with no sound.
- Devices here: "Speakers (DX1 II)" is the USB DAC (accepts only 32-bit words
  with 24 valid bits; 44.1–96 kHz verified). The default device "BlackMeow"
  refuses 24/96 in exclusive mode, which is a handy negative case.
- To drive the real UI, start the app with
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and talk
  Chrome DevTools Protocol to the page at `localhost:1420`. Native dialogs
  (file and folder pickers) cannot be driven; call the command behind them.
  Clean up any playlists a test creates: they live in
  `%APPDATA%\com.mdmusic.app\playlists.json`, the user's real data.

## Gotchas

- Tailwind theme colours are named `base`, `panel`, `raised`, `line`, `ink`,
  `muted`, `accent`, `good`, `warn`, `bad` (`ui/src/app.css`). So **`text-base`
  is the dark background colour here, not a font size**. Use `text-[15px]` or
  `text-sm`/`text-lg` for size; `text-base` is only for dark text on a light button.
- `wasapi` 0.27 and `symphonia` 0.6 differ from older examples online. Read the
  crate source under `~/.cargo/registry/src` instead of guessing.
- In lofty, ID3v2 lyrics (USLT) are `ItemKey::UnsyncLyrics`; Vorbis and MP4
  lyrics are `ItemKey::Lyrics`. Read both.
- Cover art reaches the UI through the `cover://` scheme (`coverUrl()` in
  `api.ts`), served by `cover_response` in `src-tauri/src/lib.rs`. A new image
  source needs its host added to `img-src` in `tauri.conf.json`.
- Serde names: Rust struct fields stay snake_case in JSON (`duration_ms`);
  command arguments are camelCase on the JS side (`deviceId`).
- Tauri is pinned to v2. v3 exists only as an alpha.
- Uses npm workspaces, not pnpm.
- In this environment the Bash tool breaks on heredocs containing backticks or
  odd quotes. Write files with the Write tool instead.
- A terminal opened before Rust was installed has no `cargo` on PATH. Prefix
  with `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"` or restart VS Code.

## Not built yet

SQLite library index and search, per-track playlists and reordering, cover
thumbnails in lists, tags for DSF/DFF, media keys and tray, volume control,
automatic reconnect when the DAC is replugged, ASIO output for native DSD,
remembering the output device between launches.
