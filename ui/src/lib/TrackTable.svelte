<script lang="ts">
  // A list of tracks. Double-click a row to play from there; right-click
  // (or the menu key) for the track menu. With `onsort`, the column headers
  // are buttons that sort the list. With `onmove`, rows can be dragged to a
  // new place.
  //
  // Dragging uses pointer events, not HTML drag and drop: Tauri's file drop
  // handler takes over the webview's drag events.
  import { onDestroy } from "svelte";
  import { formatTime, type TrackMeta } from "./api";
  import type { SortKey, TrackSort } from "./sort";
  import SourceMark from "./SourceMark.svelte";

  let {
    tracks,
    currentPath,
    playing,
    onplay,
    onmenu,
    menuIndex = null,
    source,
    sort = null,
    onsort,
    onmove,
  }: {
    tracks: TrackMeta[];
    /** Path of the track being heard, to highlight its row. */
    currentPath: string | null;
    playing: boolean;
    onplay: (index: number) => void;
    onmenu: (index: number, x: number, y: number) => void;
    /** Row whose menu is open, kept highlighted while it is. */
    menuIndex?: number | null;
    /** In a playlist: how each track got there. Draws a mark before the title. */
    source?: (track: TrackMeta) => "folder" | "file";
    /** The column `tracks` is sorted by, to mark its header. `null` is the list's own order. */
    sort?: TrackSort | null;
    /** A header was clicked: a column to sort by, or `null` (the # column) for the list's own order. */
    onsort?: (key: SortKey | null) => void;
    /** A row was dragged so that the track at `from` ends up at `to`. */
    onmove?: (from: number, to: number) => void;
  } = $props();

  /** Pixels the pointer must travel before a press becomes a drag. */
  const DRAG_THRESHOLD = 5;
  /** Near this many pixels from the list's top or bottom edge, a drag scrolls it. */
  const SCROLL_EDGE = 48;
  const SCROLL_SPEED = 14;

  /** A row being dragged: where it was, and the gap it would drop into (0 to tracks.length). */
  let drag = $state<{ from: number; slot: number; startY: number; active: boolean; pointer: number } | null>(
    null,
  );
  let rows = $state<HTMLElement[]>([]);
  let lastY = 0;
  let scrollFrame = 0;

  /** The gap between rows that a pointer at `y` points to. */
  function slotAt(y: number): number {
    for (let i = 0; i < tracks.length; i++) {
      const rect = rows[i]?.getBoundingClientRect();
      if (rect && y < rect.top + rect.height / 2) return i;
    }
    return tracks.length;
  }

  /** Scroll the list while the pointer is held near its edge. */
  function scrollStep() {
    if (!drag?.active) return;
    const scroller = rows[0]?.closest("main");
    if (scroller) {
      const box = scroller.getBoundingClientRect();
      const by = lastY < box.top + SCROLL_EDGE ? -SCROLL_SPEED : lastY > box.bottom - SCROLL_EDGE ? SCROLL_SPEED : 0;
      if (by !== 0) {
        scroller.scrollBy(0, by);
        drag.slot = slotAt(lastY);
      }
    }
    scrollFrame = requestAnimationFrame(scrollStep);
  }

  function onPointerDown(e: PointerEvent, index: number) {
    if (!onmove || e.button !== 0) return;
    drag = { from: index, slot: index, startY: e.clientY, active: false, pointer: e.pointerId };
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.pointer) return;
    lastY = e.clientY;
    if (!drag.active) {
      if (Math.abs(e.clientY - drag.startY) < DRAG_THRESHOLD) return;
      drag.active = true;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      scrollFrame = requestAnimationFrame(scrollStep);
    }
    drag.slot = slotAt(e.clientY);
  }

  function endDrag(e: PointerEvent, commit: boolean) {
    if (!drag || e.pointerId !== drag.pointer) return;
    const { from, slot, active } = drag;
    drag = null;
    cancelAnimationFrame(scrollFrame);
    if (!commit || !active) return;
    // Dropping below its own place, the row's old spot closes up first.
    const to = slot > from ? slot - 1 : slot;
    if (to !== from) onmove?.(from, to);
  }

  onDestroy(() => cancelAnimationFrame(scrollFrame));

  /** Draw the drop line above row `index` (or below the last row). */
  function dropLine(index: number): string {
    if (!drag?.active || drag.slot === drag.from || drag.slot === drag.from + 1) return "";
    if (drag.slot === index) return "shadow-[inset_0_2px_0_var(--color-accent)]";
    if (index === tracks.length - 1 && drag.slot === tracks.length) return "shadow-[inset_0_-2px_0_var(--color-accent)]";
    return "";
  }
</script>

{#snippet heading(label: string, key: SortKey, right = false)}
  {#if onsort}
    {@const active = sort?.key === key}
    <button
      class="flex min-w-0 items-center gap-1 uppercase tracking-wider hover:text-ink {right
        ? 'justify-end'
        : ''} {active ? 'text-ink' : ''}"
      title={active
        ? sort?.desc
          ? "Sorted, reversed. Click for the playlist's own order"
          : "Sorted. Click to reverse"
        : `Sort by ${label.toLowerCase()}`}
      onclick={() => onsort(key)}
    >
      <span class="truncate">{label}</span>
      {#if active}<span class="text-[9px] text-accent" aria-hidden="true">{sort?.desc ? "▼" : "▲"}</span>{/if}
    </button>
  {:else}
    <span class={right ? "text-right" : ""}>{label}</span>
  {/if}
{/snippet}

<div class="px-3 pb-4 {drag?.active ? 'cursor-grabbing' : ''}">
  <div
    class="grid grid-cols-[2rem_minmax(0,5fr)_minmax(0,3fr)_minmax(0,3fr)_3.5rem] gap-4 border-b border-line px-3 pb-2 text-xs uppercase tracking-wider text-muted"
  >
    {#if onsort}
      <button
        class="text-right hover:text-ink disabled:hover:text-muted"
        title={sort ? "Back to the playlist's own order" : "In the playlist's own order"}
        aria-label="Playlist order"
        disabled={!sort}
        onclick={() => onsort(null)}
      >
        #
      </button>
    {:else}
      <span class="text-right">#</span>
    {/if}
    {@render heading("Title", "title")}
    {@render heading("Artist", "artist")}
    {@render heading("Album", "album")}
    {@render heading("Time", "time", true)}
  </div>
  {#each tracks as track, index (index)}
    {@const active = track.path === currentPath}
    <button
      bind:this={rows[index]}
      class="grid w-full grid-cols-[2rem_minmax(0,5fr)_minmax(0,3fr)_minmax(0,3fr)_3.5rem] items-center gap-4 rounded-lg px-3 py-2 text-left text-sm {drag?.active
        ? ''
        : 'hover:bg-raised'} {active || menuIndex === index ? 'bg-raised' : ''} {menuIndex === index
        ? 'ring-1 ring-accent/50'
        : ''} {drag?.active && drag.from === index ? 'opacity-40' : ''} {dropLine(index)}"
      onpointerdown={(e) => onPointerDown(e, index)}
      onpointermove={onPointerMove}
      onpointerup={(e) => endDrag(e, true)}
      onpointercancel={(e) => endDrag(e, false)}
      ondblclick={() => onplay(index)}
      oncontextmenu={(e) => {
        e.preventDefault();
        // The keyboard menu key reports (0, 0): open by the row instead.
        const fromKeyboard = e.clientX === 0 && e.clientY === 0;
        const rect = e.currentTarget.getBoundingClientRect();
        onmenu(index, fromKeyboard ? rect.left + 48 : e.clientX, fromKeyboard ? rect.bottom : e.clientY);
      }}
      title={onmove
        ? "Double-click to play, drag to move, right-click for more"
        : "Double-click to play, right-click for more"}
    >
      <span class="text-right tabular-nums {active ? 'text-accent' : 'text-muted'}">
        {active && playing ? "▶" : index + 1}
      </span>
      <span class="flex min-w-0 items-center gap-2">
        {#if source}
          <SourceMark kind={source(track)} />
        {/if}
        <span class="truncate {active ? 'font-medium text-accent' : ''}">{track.title}</span>
      </span>
      <span class="truncate text-muted">{track.artist ?? ""}</span>
      <span class="truncate text-muted">{track.album ?? ""}</span>
      <span class="text-right tabular-nums text-muted">
        {track.duration_ms !== null ? formatTime(track.duration_ms) : ""}
      </span>
    </button>
  {/each}
</div>
