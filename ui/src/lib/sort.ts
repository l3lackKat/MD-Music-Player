// Sorting a playlist's track list by a column.
//
// A sort only changes how the list is shown and the order it is played in;
// the playlist itself keeps its own order (folders in album order, then the
// single files). The choice is remembered per playlist in localStorage.

import type { TrackMeta } from "./api";

export type SortKey = "title" | "artist" | "album" | "time";

export interface TrackSort {
  key: SortKey;
  /** Z to A, or longest first. */
  desc: boolean;
}

/** The sort of each playlist that has one, by playlist id. */
export type PlaylistSorts = Record<number, TrackSort>;

const STORAGE_KEY = "playlistSort";
const KEYS: SortKey[] = ["title", "artist", "album", "time"];

// Numbers inside names sort by value ("Track 2" before "Track 10"); case and accents are ignored.
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function value(track: TrackMeta, key: SortKey): string | number | null {
  if (key === "time") return track.duration_ms;
  // An empty tag is no tag.
  return track[key] || null;
}

/**
 * The tracks in sorted order, as a new array. Tracks that tie keep the order
 * they came in, so sorting by album leaves each album in track order. Tracks
 * with nothing in the column go last, in both directions.
 */
export function sortTracks(tracks: TrackMeta[], sort: TrackSort | null): TrackMeta[] {
  if (!sort) return tracks;
  const direction = sort.desc ? -1 : 1;
  return [...tracks].sort((a, b) => {
    const x = value(a, sort.key);
    const y = value(b, sort.key);
    if (x === null || y === null) return x === y ? 0 : x === null ? 1 : -1;
    const order = typeof x === "number" && typeof y === "number" ? x - y : collator.compare(String(x), String(y));
    return order * direction;
  });
}

/** What a click on a column header leads to: ascending, then descending, then the playlist's own order. */
export function nextSort(current: TrackSort | null, key: SortKey): TrackSort | null {
  if (current?.key !== key) return { key, desc: false };
  return current.desc ? null : { key, desc: true };
}

export function loadSorts(): PlaylistSorts {
  const sorts: PlaylistSorts = {};
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    if (typeof saved !== "object" || saved === null) return sorts;
    for (const [id, sort] of Object.entries(saved as Record<string, Partial<TrackSort> | null>)) {
      if (sort && KEYS.includes(sort.key as SortKey)) sorts[Number(id)] = { key: sort.key!, desc: sort.desc === true };
    }
  } catch {}
  return sorts;
}

export function saveSorts(sorts: PlaylistSorts) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(sorts));
  } catch {}
}
