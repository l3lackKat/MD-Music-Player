<script lang="ts">
  // A list of tracks, each with its small cover.
  //
  // - Click selects a row, Ctrl+click adds one, Shift+click a range, Ctrl+A
  //   all, Escape none. Double-click (or Enter) plays.
  // - Right-click (or the menu key) opens the menu for the selected rows.
  // - Delete asks to remove the selected rows (`ondelete`).
  // - With `onsort`, the column headers sort the list.
  // - With `onmove`, rows can be dragged to a new place; dragging a selected
  //   row takes the whole selection.
  import { onDestroy } from "svelte";
  import { formatTime, type TrackMeta } from "./api";
  import { RowDrag } from "./reorder.svelte";
  import { Selection } from "./selection.svelte";
  import type { SortKey, TrackSort } from "./sort";
  import SourceMark from "./SourceMark.svelte";
  import Thumb from "./Thumb.svelte";

  let {
    tracks,
    currentPath,
    playing,
    onplay,
    onmenu,
    source,
    sort = null,
    onsort,
    onmove,
    ondelete,
  }: {
    tracks: TrackMeta[];
    /** Path of the track being heard, to highlight its row. */
    currentPath: string | null;
    playing: boolean;
    onplay: (index: number) => void;
    /** The menu for these rows (the selection, in list order) at a screen position. */
    onmenu: (rows: number[], x: number, y: number) => void;
    /** In a playlist: how each track got there. Draws a mark before the title. */
    source?: (track: TrackMeta) => "folder" | "file";
    /** The column `tracks` is sorted by, to mark its header. `null` is the list's own order. */
    sort?: TrackSort | null;
    /** A header was clicked: a column to sort by, or `null` (the # column) for the list's own order. */
    onsort?: (key: SortKey | null) => void;
    /** `rows` were dragged into the gap before `slot` (0 to tracks.length). */
    onmove?: (rows: number[], slot: number) => void;
    /** Delete was pressed with these rows selected. */
    ondelete?: (rows: number[]) => void;
  } = $props();

  const selection = new Selection<TrackMeta>();
  let rowElements = $state<HTMLElement[]>([]);

  const drag = new RowDrag({
    rows: () => rowElements.slice(0, tracks.length),
    scroller: () => rowElements[0]?.closest("main"),
    drop: (rows, slot) => {
      const moved = rows.map((i) => tracks[i]);
      onmove?.(rows, slot);
      // The moved rows stay selected in their new place.
      selection.set(moved);
    },
  });
  onDestroy(() => drag.destroy());

  /** The rows a press on `index` would drag: the selection if it is part of it. */
  function rowsFor(index: number): number[] {
    return selection.has(tracks[index]) ? selection.rows(tracks) : [index];
  }

  function openMenu(e: MouseEvent, index: number) {
    e.preventDefault();
    if (!selection.has(tracks[index])) selection.only(tracks[index]);
    // The keyboard menu key reports (0, 0): open by the row instead.
    const fromKeyboard = e.clientX === 0 && e.clientY === 0;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    onmenu(selection.rows(tracks), fromKeyboard ? rect.left + 48 : e.clientX, fromKeyboard ? rect.bottom : e.clientY);
  }

  function onKey(e: KeyboardEvent) {
    const rows = selection.rows(tracks);
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
      e.preventDefault();
      selection.all(tracks);
    } else if (e.key === "Escape" && rows.length > 0) {
      selection.clear();
    } else if (e.key === "Delete" && rows.length > 0 && ondelete) {
      e.preventDefault();
      ondelete(rows);
    } else if (e.key === "Enter" && rows.length > 0) {
      e.preventDefault();
      onplay(rows[0]);
    }
  }

  const ROW_GRID = "grid-cols-[2rem_minmax(0,5fr)_minmax(0,3fr)_minmax(0,3fr)_3.5rem]";
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

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="px-3 pb-4 {drag.active ? 'cursor-grabbing' : ''}" onkeydown={onKey}>
  <div class="grid {ROW_GRID} gap-4 border-b border-line px-3 pb-2 text-xs uppercase tracking-wider text-muted">
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
    {@const selected = selection.has(track)}
    {@const line = drag.line(index)}
    <button
      bind:this={rowElements[index]}
      data-row
      aria-pressed={selected}
      class="grid w-full {ROW_GRID} items-center gap-4 rounded-lg px-3 py-1.5 text-left text-sm {selected
        ? 'bg-accent/15'
        : active
          ? 'bg-raised'
          : drag.active
            ? ''
            : 'hover:bg-raised'} {drag.active && drag.dragging.includes(index) ? 'opacity-40' : ''} {line === 'above'
        ? 'shadow-[inset_0_2px_0_var(--color-accent)]'
        : line === 'below'
          ? 'shadow-[inset_0_-2px_0_var(--color-accent)]'
          : ''}"
      onclick={(e) => {
        if (!drag.endedDrag()) selection.click(tracks, index, e);
      }}
      ondblclick={() => onplay(index)}
      oncontextmenu={(e) => openMenu(e, index)}
      onpointerdown={onmove ? (e) => drag.down(e, rowsFor(index)) : undefined}
      onpointermove={onmove ? (e) => drag.move(e) : undefined}
      onpointerup={onmove ? (e) => drag.up(e) : undefined}
      onpointercancel={onmove ? (e) => drag.cancel(e) : undefined}
      title={onmove
        ? "Double-click to play, drag to move, right-click for more"
        : "Double-click to play, right-click for more"}
    >
      <span class="text-right tabular-nums {active ? 'text-accent' : 'text-muted'}">
        {active && playing ? "▶" : index + 1}
      </span>
      <span class="flex min-w-0 items-center gap-2.5">
        <Thumb path={track.path} class="h-8 w-8" />
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
