// Typed wrappers around the Rust commands and events in src-tauri/src/lib.rs.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type PlayState = "stopped" | "playing" | "paused";
export type OutputMode = "exclusive" | "shared";

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
  /** The device's volume works in exclusive mode. Without it, shared mode is used. */
  exclusive_available: boolean;
  /** The device was at 100% when chosen (or at launch) and was turned down to 10% first. */
  volume_lowered: boolean;
}

/** The Windows volume of the selected output device. */
export interface DeviceVolume {
  /** 0 to 1, on the same scale as the Windows slider. */
  level: number;
  muted: boolean;
  /** The device applies the level itself, so it also works in exclusive mode. */
  hardware: boolean;
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
  playlistRemoveFile: (id: number, file: string) =>
    invoke<Playlist>("playlist_remove_file", { id, file }),
  showInFolder: (path: string) => invoke<void>("show_in_folder", { path }),
  appendFiles: (paths: string[]) => invoke<void>("append_files", { paths }),
  setShuffle: (on: boolean) => invoke<void>("set_shuffle", { on }),
  playlistRemoveFolder: (id: number, folder: string) =>
    invoke<Playlist>("playlist_remove_folder", { id, folder }),
  playlistScan: (id: number) => invoke<TrackMeta[]>("playlist_scan", { id }),

  listDevices: () => invoke<DeviceInfo[]>("list_devices"),
  getOutput: () => invoke<OutputSettings>("get_output"),
  setOutput: (deviceId: string | null, mode: OutputMode) =>
    invoke<OutputSettings>("set_output", { deviceId, mode }),
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
  position: (position: PositionEvent) => void;
  error: (message: string) => void;
  queueEnded: () => void;
}

/** Subscribe to every player event. Returns a function that unsubscribes. */
export async function listenPlayer(handlers: PlayerHandlers): Promise<UnlistenFn> {
  const unlisten = await Promise.all([
    listen<{ state: PlayState }>("player://state", (e) => handlers.state(e.payload.state)),
    listen<TrackEvent>("player://track", (e) => handlers.track(e.payload)),
    listen<PositionEvent>("player://position", (e) => handlers.position(e.payload)),
    listen<{ message: string }>("player://error", (e) => handlers.error(e.payload.message)),
    listen("player://queue-ended", () => handlers.queueEnded()),
  ]);
  return () => unlisten.forEach((u) => u());
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
  const khz = hz / 1000;
  return `${Number.isInteger(khz) ? khz : khz.toFixed(1)} kHz`;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
