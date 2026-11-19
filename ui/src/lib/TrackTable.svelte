<script lang="ts">
  // A list of tracks, each with its small cover.
  //
  // - Click selects a row, Ctrl+click adds one, Shift+click a range, Ctrl+A
  //   all, Escape none. Double-click (or Enter) plays.
  // - The list is one Tab stop. Arrow keys, Page Up/Down, Home and End move
  //   through it and select (with Shift, a range).
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
    /** In a playlist: how each track got there. Single files get a mark after the title. */
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
  /** The row last clicked or moved to by key. It is the list's one Tab stop. */
  let cursor = $state(0);
  const tabRow = $derived(Math.max(0, Math.min(cursor, tracks.length - 1)));
  /** How far Page Up and Page Down move. */
  const PAGE_ROWS = 10;

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

  /** Where an arrow, paging or Home/End key goes from row `from`, or null for other keys. */
  function keyTarget(key: string, from: number): number | null {
    const last = tracks.length - 1;
    const to =
      key === "ArrowDown" ? from + 1
      : key === "ArrowUp" ? from - 1
      : key === "PageDown" ? from + PAGE_ROWS
      : key === "PageUp" ? from - PAGE_ROWS
      : key === "Home" ? 0
      : key === "End" ? last
      : null;
    return to === null ? null : Math.max(0, Math.min(to, last));
  }

  function onKey(e: KeyboardEvent) {
    const rows = selection.rows(tracks);
    const row = (e.target as HTMLElement).closest("[data-row]");
    const to = row && !e.altKey ? keyTarget(e.key, rowElements.indexOf(row as HTMLElement)) : null;
    if (to !== null) {
      e.preventDefault();
      selection.click(tracks, to, { ctrlKey: false, metaKey: false, shiftKey: e.shiftKey });
      cursor = to;
      rowElements[to]?.focus();
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
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
      class="group grid w-full {ROW_GRID} items-center gap-4 rounded-lg px-3 py-1.5 text-left text-sm {selected
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
      tabindex={index === tabRow ? 0 : -1}
      onclick={(e) => {
        cursor = index;
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
      <!-- The # cell: the number, bars while this track plays, and a play button on hover
           (Enter plays from the keyboard, so the button is for the mouse only). -->
      <span class="relative flex h-5 items-center justify-end tabular-nums {active ? 'text-accent' : 'text-muted'}">
        {#if active && playing}
          <span class="playing-bars" aria-hidden="true"><span></span><span></span><span></span></span>
          <span class="sr-only">Playing</span>
        {:else}
          <span class={active ? "" : "group-hover:invisible"}>{index + 1}</span>
          {#if !active && !drag.active}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <span
              class="absolute inset-y-0 right-0 hidden w-5 place-items-center rounded text-ink hover:text-accent group-hover:grid"
              title="Play"
              aria-hidden="true"
              onclick={(e) => {
                e.stopPropagation();
                onplay(index);
              }}
              ondblclick={(e) => e.stopPropagation()}
            >
              <svg viewBox="0 0 24 24" class="h-3.5 w-3.5" fill="currentColor"><path d="M7 4.5v15l12.5-7.5z" /></svg>
            </span>
          {/if}
        {/if}
      </span>
      <span class="flex min-w-0 items-center gap-2.5">
        <Thumb path={track.path} class="h-8 w-8" />
        <!-- Each cell's own text as its tooltip, so a cut-off name can still be read. -->
        <span class="truncate {active ? 'font-medium text-accent' : ''}" title={track.title}>{track.title}</span>
        <!-- Only the exception is marked: a track added on its own, not found in a folder. -->
        {#if source?.(track) === "file"}
          <SourceMark kind="file" />
        {/if}
      </span>
      <span class="truncate text-muted" title={track.artist ?? undefined}>{track.artist ?? ""}</span>
      <span class="truncate text-muted" title={track.album ?? undefined}>{track.album ?? ""}</span>
      <span class="text-right tabular-nums text-muted">
        {track.duration_ms !== null ? formatTime(track.duration_ms) : ""}
      </span>
    </button>
  {/each}
</div>
