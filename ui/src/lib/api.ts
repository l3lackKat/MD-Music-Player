// Typed wrappers around the Rust commands and events in src-tauri/src/lib.rs.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type PlayState = "stopped" | "playing" | "paused";
export type OutputMode = "exclusive" | "shared";
/** After the last track: stop, start over, or (`one`) play the current track again. */
export type Repeat = "off" | "all" | "one";

export interface DeviceInfo {
  id: string;
  name: string;
  is_default: boolean;
}

export interface OutputSettings {
  device_id: string | null;
  /** The mode in use. */
  mode: OutputMode;
  /** The mode the user picked; differs from `mode` while exclusive is unavailable. */
  wanted: OutputMode;
  /** Exclusive mode can be used: the device has a hardware volume, or the software volume is on. */
  exclusive_available: boolean;
  /** The device has an adjustable volume of its own. */
  hardware_volume: boolean;
  /** The user turned the software volume on (it only acts on devices without a hardware volume). */
  soft_volume_enabled: boolean;
  /** The software volume is in use now: the volume bar scales samples, so playback is not bit-perfect. */
  soft_volume: boolean;
  /** The device was at 100% when chosen (or at launch) and was turned down to 10% first. */
  volume_lowered: boolean;
  /** At launch: the saved device is not connected, so the system default is used for now. */
  missing_device: string | null;
}

/** The device that went away is back and playing again. */
export interface Reconnected {
  output: OutputSettings;
  device: string;
}

/** The Windows volume of the selected output device. */
export interface DeviceVolume {
  /** 0 to 1, on the same scale as the Windows slider. */
  level: number;
  muted: boolean;
  /** The device applies the level itself, so it also works in exclusive mode. */
  hardware: boolean;
  /** This is MDMusic's software volume, which scales the samples, not the Windows one. */
  software: boolean;
}

export type SampleKind = { type: "pcm"; bits: number } | { type: "float" } | { type: "dsd" };

export interface SignalPath {
  source: {
    codec: string;
    kind: SampleKind;
    sample_rate: number;
    channels: number;
    duration_ms: number | null;
  };
  output: {
    sample_rate: number;
    channels: number;
    container: "i16" | "i24" | "i32";
    valid_bits: number;
    dop: boolean;
  };
  device: string;
  exclusive: boolean;
  bit_perfect: boolean;
  /** The equalizer is changing this track's samples. */
  eq: boolean;
  /** The software volume is scaling this track's samples. */
  soft_volume: boolean;
}

export interface EqBand {
  freq_hz: number;
  gain_db: number;
  q: number;
}

/** The opt-in equalizer. While it is on, PCM tracks are not bit-perfect; DSD passes untouched. */
export interface EqSettings {
  enabled: boolean;
  /** Gain before the bands. Lower it to make room for boosts. */
  preamp_db: number;
  bands: EqBand[];
}

/** Everything known about one file, for the track info panel. */
export interface TrackDetails {
  path: string;
  file_name: string;
  folder: string;
  size_bytes: number | null;
  /** Milliseconds since 1970. */
  modified_ms: number | null;
  audio: {
    container: string | null;
    codec: string | null;
    bits: number | null;
    lossy_or_float: boolean;
    dsd: boolean;
    sample_rate: number | null;
    channels: number | null;
    duration_ms: number | null;
    bitrate_kbps: number | null;
    error: string | null;
  };
  tags: { kind: string; fields: { key: string; value: string }[] }[];
  pictures: { kind: string; mime: string | null; width: number | null; height: number | null; size_bytes: number }[];
  lyrics: { synced: boolean; lines: number } | null;
}

export interface TrackEvent {
  index: number;
  path: string;
  signal_path: SignalPath;
}

export interface PositionEvent {
  ms: number;
  duration_ms: number | null;
}

/** A track with its tags, as read by mdm-library. */
export interface TrackMeta {
  path: string;
  /** Tag title, or the file name when the file has none. */
  title: string;
  artist: string | null;
  album: string | null;
  album_artist: string | null;
  track_no: number | null;
  disc_no: number | null;
  year: number | null;
  duration_ms: number | null;
}

export interface Playlist {
  id: number;
  name: string;
  folders: string[];
  /** Tracks added one by one, after the folders' tracks. */
  files: string[];
}

/** Does the playlist hold this file on its own (not through a folder)? */
export function holdsFile(playlist: Playlist, path: string): boolean {
  const wanted = path.toLowerCase();
  return playlist.files.some((f) => f.toLowerCase() === wanted);
}

export interface Lyrics {
  /** Every line has a time, in order. */
  synced: boolean;
  lines: { time_ms: number | null; text: string }[];
}

/** URL of a track's cover image. It answers 404 when there is none. */
export function coverUrl(path: string): string {
  return convertFileSrc(path, "cover");
}

/** URL of a track's small cover (JPEG, 96 px), made once and kept in the library index. 404 when there is none. */
export function thumbUrl(path: string): string {
  return `${coverUrl(path)}?thumb`;
}

/** How many tracks the library index holds: every playlist's tracks. */
export interface LibraryStatus {
  tracks: number;
}

export const api = {
  expandPaths: (paths: string[]) => invoke<TrackMeta[]>("expand_paths", { paths }),
  getLyrics: (path: string) => invoke<Lyrics | null>("get_lyrics", { path }),
  trackDetails: (path: string) => invoke<TrackDetails>("track_details", { path }),
  getEq: () => invoke<EqSettings>("get_eq"),
  /** Apply and save. Returns the settings as applied (values clamped into range). */
  setEq: (settings: EqSettings) => invoke<EqSettings>("set_eq", { settings }),
  playlists: () => invoke<Playlist[]>("playlists"),
  playlistCreate: (name: string) => invoke<Playlist>("playlist_create", { name }),
  playlistRename: (id: number, name: string) => invoke<Playlist>("playlist_rename", { id, name }),
  playlistDelete: (id: number) => invoke<void>("playlist_delete", { id }),
  /** Add folders and audio files; `skipped` lists paths that were neither. */
  playlistAdd: (id: number, paths: string[]) =>
    invoke<{ playlist: Playlist; skipped: string[] }>("playlist_add", { id, paths }),
  /** Remove single files; tracks that come from a folder stay. */
  playlistRemoveFiles: (id: number, files: string[]) =>
    invoke<Playlist>("playlist_remove_files", { id, files }),
  /** Put the playlists in this order; returns them all. */
  playlistReorder: (ids: number[]) => invoke<Playlist[]>("playlist_reorder", { ids }),
  /** Tracks of any playlist matching every word, accents and case ignored. */
  search: (query: string) => invoke<TrackMeta[]>("search", { query }),
  libraryStatus: () => invoke<LibraryStatus>("library_status"),
  showInFolder: (path: string) => invoke<void>("show_in_folder", { path }),
  appendFiles: (paths: string[]) => invoke<void>("append_files", { paths }),
  /** Take tracks out of the engine's queue; remove them from the UI's `queue` too. */
  queueRemove: (indexes: number[]) => invoke<void>("queue_remove", { indexes }),
  /** Put the engine's queue in a new order, `order[new] = old`; reorder the UI's `queue` the same way. */
  queueReorder: (order: number[]) => invoke<void>("queue_reorder", { order }),
  setShuffle: (on: boolean) => invoke<void>("set_shuffle", { on }),
  setRepeat: (repeat: Repeat) => invoke<void>("set_repeat", { repeat }),
  playlistRemoveFolder: (id: number, folder: string) =>
    invoke<Playlist>("playlist_remove_folder", { id, folder }),
  playlistScan: (id: number) => invoke<TrackMeta[]>("playlist_scan", { id }),

  listDevices: () => invoke<DeviceInfo[]>("list_devices"),
  getOutput: () => invoke<OutputSettings>("get_output"),
  setOutput: (deviceId: string | null, mode: OutputMode) =>
    invoke<OutputSettings>("set_output", { deviceId, mode }),
  /** Turn the software volume on or off; returns the output as it now is. */
  setSoftVolume: (enabled: boolean) => invoke<OutputSettings>("set_soft_volume", { enabled }),
  getVolume: () => invoke<DeviceVolume>("get_volume"),
  /** Change the level (0 to 1), the mute, or both. Returns the new state. */
  setVolume: (change: { level?: number; muted?: boolean }) =>
    invoke<DeviceVolume>("set_volume", { level: change.level ?? null, muted: change.muted ?? null }),
  supportedExtensions: () => invoke<string[]>("supported_extensions"),
  playFiles: (paths: string[], index: number) => invoke<void>("play_files", { paths, index }),
  play: () => invoke<void>("play"),
  pause: () => invoke<void>("pause"),
  stop: () => invoke<void>("stop"),
  seek: (ms: number) => invoke<void>("seek", { ms: Math.round(ms) }),
  next: () => invoke<void>("next"),
  previous: () => invoke<void>("previous"),
};

export interface PlayerHandlers {
  state: (state: PlayState) => void;
  track: (track: TrackEvent) => void;
  /** The queue was edited: the track being heard is now at `index`. */
  current: (index: number) => void;
  position: (position: PositionEvent) => void;
  error: (message: string) => void;
  /** The device stopped while playing. Playback stopped where it was. */
  deviceLost: (message: string) => void;
  queueEnded: () => void;
}

/** Subscribe to every player event. Returns a function that unsubscribes. */
export async function listenPlayer(handlers: PlayerHandlers): Promise<UnlistenFn> {
  const unlisten = await Promise.all([
    listen<{ state: PlayState }>("player://state", (e) => handlers.state(e.payload.state)),
    listen<TrackEvent>("player://track", (e) => handlers.track(e.payload)),
    listen<{ index: number }>("player://current", (e) => handlers.current(e.payload.index)),
    listen<PositionEvent>("player://position", (e) => handlers.position(e.payload)),
    listen<{ message: string }>("player://error", (e) => handlers.error(e.payload.message)),
    listen<{ message: string }>("player://device-lost", (e) => handlers.deviceLost(e.payload.message)),
    listen("player://queue-ended", () => handlers.queueEnded()),
  ]);
  return () => unlisten.forEach((u) => u());
}

export interface OutputHandlers {
  /** The device that stopped is unplugged; playback resumes when it is back. */
  waiting: (device: string) => void;
  reconnected: (event: Reconnected) => void;
}

/** Follow a device that went away and came back. Returns a function that unsubscribes. */
export async function listenOutput(handlers: OutputHandlers): Promise<UnlistenFn> {
  const unlisten = await Promise.all([
    listen<string>("output://waiting", (e) => handlers.waiting(e.payload)),
    listen<Reconnected>("output://reconnected", (e) => handlers.reconnected(e.payload)),
  ]);
  return () => unlisten.forEach((u) => u());
}

/** The library index finished a refresh (after launch, or after the playlists changed). */
export function listenLibrary(handler: (status: LibraryStatus) => void): Promise<UnlistenFn> {
  return listen<LibraryStatus>("library://updated", (e) => handler(e.payload));
}

/** Called whenever the selected device's Windows volume changes, wherever it was changed. */
export function listenVolume(handler: (volume: DeviceVolume) => void): Promise<UnlistenFn> {
  return listen<DeviceVolume>("volume://changed", (e) => handler(e.payload));
}

/** "3:07" or "1:02:45". */
export function formatTime(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** "44.1 kHz", "192 kHz". */
export function formatRate(hz: number): string {
  return `${formatKhz(hz)} kHz`;
}

/** "44.1", "192": a rate in kHz without the unit, as in "24/96". */
export function formatKhz(hz: number): string {
  const khz = hz / 1000;
  return `${Number.isInteger(khz) ? khz : khz.toFixed(1)}`;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
