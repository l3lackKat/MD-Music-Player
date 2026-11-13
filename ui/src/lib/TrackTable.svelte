<script lang="ts">
  // A list of tracks. Double-click a row to play from there; right-click
  // (or the menu key) for the track menu.
  import { formatTime, type TrackMeta } from "./api";

  let {
    tracks,
    currentPath,
    playing,
    onplay,
    onmenu,
    menuIndex = null,
  }: {
    tracks: TrackMeta[];
    /** Path of the track being heard, to highlight its row. */
    currentPath: string | null;
    playing: boolean;
    onplay: (index: number) => void;
    onmenu: (index: number, x: number, y: number) => void;
    /** Row whose menu is open, kept highlighted while it is. */
    menuIndex?: number | null;
  } = $props();
</script>

<div class="px-3 pb-4">
  <div
    class="grid grid-cols-[2rem_minmax(0,5fr)_minmax(0,3fr)_minmax(0,3fr)_3.5rem] gap-4 border-b border-line px-3 pb-2 text-xs uppercase tracking-wider text-muted"
  >
    <span class="text-right">#</span>
    <span>Title</span>
    <span>Artist</span>
    <span>Album</span>
    <span class="text-right">Time</span>
  </div>
  {#each tracks as track, index (index)}
    {@const active = track.path === currentPath}
    <button
      class="grid w-full grid-cols-[2rem_minmax(0,5fr)_minmax(0,3fr)_minmax(0,3fr)_3.5rem] items-center gap-4 rounded-lg px-3 py-2 text-left text-sm hover:bg-raised {active ||
      menuIndex === index
        ? 'bg-raised'
        : ''} {menuIndex === index ? 'ring-1 ring-accent/50' : ''}"
      ondblclick={() => onplay(index)}
      oncontextmenu={(e) => {
        e.preventDefault();
        // The keyboard menu key reports (0, 0): open by the row instead.
        const fromKeyboard = e.clientX === 0 && e.clientY === 0;
        const rect = e.currentTarget.getBoundingClientRect();
        onmenu(index, fromKeyboard ? rect.left + 48 : e.clientX, fromKeyboard ? rect.bottom : e.clientY);
      }}
      title="Double-click to play, right-click for more"
    >
      <span class="text-right tabular-nums {active ? 'text-accent' : 'text-muted'}">
        {active && playing ? "▶" : index + 1}
      </span>
      <span class="truncate {active ? 'font-medium text-accent' : ''}">{track.title}</span>
      <span class="truncate text-muted">{track.artist ?? ""}</span>
      <span class="truncate text-muted">{track.album ?? ""}</span>
      <span class="text-right tabular-nums text-muted">
        {track.duration_ms !== null ? formatTime(track.duration_ms) : ""}
      </span>
    </button>
  {/each}
</div>
