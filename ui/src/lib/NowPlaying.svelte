<script lang="ts">
  // The side panel: cover, track details and lyrics for the current track.
  import { api, type Lyrics, type TrackMeta } from "./api";
  import Cover from "./Cover.svelte";

  let {
    track,
    positionMs,
    onseek,
    disc,
    playing,
    ontoggledisc,
  }: {
    track: TrackMeta | null;
    positionMs: number;
    onseek: (ms: number) => void;
    /** Show the cover as a round record that turns while `playing`. */
    disc: boolean;
    playing: boolean;
    /** The cover was clicked. */
    ontoggledisc: () => void;
  } = $props();

  let lyrics = $state<Lyrics | null>(null);
  let loading = $state(false);
  let lineElements = $state<HTMLElement[]>([]);

  // Load lyrics whenever the track changes.
  $effect(() => {
    const path = track?.path ?? null;
    lyrics = null;
    lineElements = [];
    if (path === null) return;
    loading = true;
    let stale = false;
    api
      .getLyrics(path)
      .then((result) => {
        if (!stale) lyrics = result;
      })
      .catch(() => {})
      .finally(() => {
        if (!stale) loading = false;
      });
    return () => {
      stale = true;
    };
  });

  // ---- Lyrics offset ----
  // Timed lyrics that run early or late can be moved, per track, in steps.
  // Kept in localStorage by file path; music files are never written.

  const OFFSET_KEY = "lyricsOffsets";
  const OFFSET_STEP_MS = 500;

  function loadOffsets(): Record<string, number> {
    try {
      const saved = JSON.parse(localStorage.getItem(OFFSET_KEY) ?? "{}");
      return saved && typeof saved === "object" ? saved : {};
    } catch {
      return {};
    }
  }

  let offsets = $state<Record<string, number>>(loadOffsets());
  /** How much later (positive) or earlier this track's lyrics show, in ms. */
  const offsetMs = $derived(track ? (offsets[track.path] ?? 0) : 0);

  /** Move this track's lyrics by `ms` (positive: later), or back to where the file has them (`null`). */
  function shiftLyrics(ms: number | null) {
    if (!track) return;
    const next = ms === null ? 0 : offsetMs + ms;
    const all = { ...offsets };
    if (next === 0) delete all[track.path];
    else all[track.path] = next;
    offsets = all;
    try {
      localStorage.setItem(OFFSET_KEY, JSON.stringify(all));
    } catch {}
  }

  /** "+0.5 s", "−1.5 s". */
  function formatOffset(ms: number): string {
    return `${ms > 0 ? "+" : "−"}${(Math.abs(ms) / 1000).toFixed(1)} s`;
  }

  /** Where line `time` is heard, with the offset applied. */
  function lineStart(time: number | null): number {
    return Math.max(0, (time ?? 0) + offsetMs);
  }

  // In timed lyrics, the line being sung: the last one that has started.
  const activeLine = $derived.by(() => {
    if (!lyrics?.synced) return -1;
    let active = -1;
    for (let i = 0; i < lyrics.lines.length; i++) {
      if (lineStart(lyrics.lines[i].time_ms) <= positionMs) active = i;
      else break;
    }
    return active;
  });

  // Keep the line being sung in the middle of the panel.
  $effect(() => {
    lineElements[activeLine]?.scrollIntoView({ block: "center", behavior: "smooth" });
  });

  const details = $derived(
    [track?.album, track?.year].filter((part) => part !== null && part !== undefined).join(" · "),
  );
</script>

<!-- `overflow-hidden`: whatever happens inside, nothing may spill onto the transport bar below. -->
<aside class="flex w-80 shrink-0 flex-col overflow-hidden border-l border-line bg-panel">
  <div class="shrink-0 p-5 pb-4">
    <!-- The cover keeps its full size; on short windows the lyrics below get less room. -->
    <Cover
      path={track?.path ?? null}
      class="aspect-square w-full rounded-xl shadow-lg shadow-black/40"
      {disc}
      spinning={playing}
      onclick={ontoggledisc}
      title={disc ? "Click for the square cover" : "Click to spin the cover like a record"}
    />
    <p class="mt-4 select-text truncate text-[15px] font-semibold" title={track?.title}>
      {track?.title ?? "Not playing"}
    </p>
    {#if !track}
      <p class="mt-0.5 text-sm text-muted">Play a track to see its cover and lyrics here.</p>
    {/if}
    {#if track?.artist}
      <p class="mt-0.5 select-text truncate text-sm text-muted">{track.artist}</p>
    {/if}
    {#if details}
      <p class="mt-0.5 select-text truncate text-xs text-muted">{details}</p>
    {/if}
  </div>

  <div class="flex min-h-0 flex-1 flex-col border-t border-line">
    <div class="flex h-9 items-end gap-0.5 px-5 pb-1">
      <h2 class="mr-auto pb-0.5 text-xs font-medium uppercase tracking-wider text-muted">Lyrics</h2>
      {#if lyrics?.synced}
        <!-- Lyrics that run early or late: move them for this track. -->
        <button
          class="grid h-6 w-6 place-items-center rounded text-sm text-muted hover:bg-raised hover:text-ink"
          title="Show the lyrics 0.5 s earlier"
          aria-label="Lyrics 0.5 seconds earlier"
          onclick={() => shiftLyrics(-OFFSET_STEP_MS)}
        >
          −
        </button>
        {#if offsetMs !== 0}
          <button
            class="h-6 rounded px-1 text-xs tabular-nums text-accent hover:bg-raised"
            title="The lyrics are moved for this track. Click to put them back"
            onclick={() => shiftLyrics(null)}
          >
            {formatOffset(offsetMs)}
          </button>
        {/if}
        <button
          class="grid h-6 w-6 place-items-center rounded text-sm text-muted hover:bg-raised hover:text-ink"
          title="Show the lyrics 0.5 s later"
          aria-label="Lyrics 0.5 seconds later"
          onclick={() => shiftLyrics(OFFSET_STEP_MS)}
        >
          +
        </button>
      {/if}
    </div>
    <!-- The edges fade, so lines scroll in and out softly instead of being cut. -->
    <div
      class="min-h-0 flex-1 overflow-y-auto px-5 pb-8 pt-2 [mask-image:linear-gradient(to_bottom,transparent,black_1rem,black_calc(100%-2.5rem),transparent)]"
    >
      {#if lyrics}
        {#each lyrics.lines as line, index (index)}
          {#if lyrics.synced}
            <!-- Sung lines fade back, the current one stands out, the ones to come wait in between. -->
            <button
              bind:this={lineElements[index]}
              class="block w-full rounded-md py-1.5 text-left text-[15px] leading-snug transition-colors duration-300 hover:text-ink {index ===
              activeLine
                ? 'font-semibold text-ink'
                : index < activeLine
                  ? 'text-muted'
                  : 'text-ink/70'}"
              title="Jump to this line"
              onclick={() => onseek(lineStart(line.time_ms))}
            >
              {line.text || "♪"}
            </button>
          {:else}
            <p class="min-h-3 select-text py-1 text-[15px] leading-snug text-ink/85">{line.text}</p>
          {/if}
        {/each}
      {:else if !track}
        <p class="pt-1 text-sm text-muted">Timed lyrics follow the song line by line.</p>
      {:else if !loading}
        <p class="pt-1 text-sm text-muted">
          No lyrics in this file. Put a <span class="text-ink/80">.lrc</span> file with the same name next to it to
          add them.
        </p>
      {/if}
    </div>
  </div>
</aside>
