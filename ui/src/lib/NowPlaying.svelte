<script lang="ts">
  // The side panel: cover, track details and lyrics for the current track.
  import { api, type Lyrics, type TrackMeta } from "./api";
  import Cover from "./Cover.svelte";

  let {
    track,
    positionMs,
    onseek,
  }: {
    track: TrackMeta | null;
    positionMs: number;
    onseek: (ms: number) => void;
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

  // In timed lyrics, the line being sung: the last one that has started.
  const activeLine = $derived.by(() => {
    if (!lyrics?.synced) return -1;
    let active = -1;
    for (let i = 0; i < lyrics.lines.length; i++) {
      if ((lyrics.lines[i].time_ms ?? 0) <= positionMs) active = i;
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

<aside class="flex w-80 shrink-0 flex-col border-l border-line bg-panel">
  <div class="p-5 pb-4">
    <Cover path={track?.path ?? null} class="aspect-square w-full rounded-xl shadow-lg shadow-black/40" />
    <p class="mt-4 select-text truncate text-[15px] font-semibold" title={track?.title}>
      {track?.title ?? "Not playing"}
    </p>
    {#if track?.artist}
      <p class="mt-0.5 select-text truncate text-sm text-muted">{track.artist}</p>
    {/if}
    {#if details}
      <p class="mt-0.5 select-text truncate text-xs text-muted">{details}</p>
    {/if}
  </div>

  <div class="flex min-h-0 flex-1 flex-col border-t border-line">
    <h2 class="px-5 pb-1 pt-3 text-xs font-medium uppercase tracking-wider text-muted">Lyrics</h2>
    <div class="min-h-0 flex-1 overflow-y-auto px-5 pb-5">
      {#if lyrics}
        {#each lyrics.lines as line, index (index)}
          {#if lyrics.synced}
            <button
              bind:this={lineElements[index]}
              class="block w-full py-1 text-left text-sm leading-snug transition-colors {index === activeLine
                ? 'font-medium text-ink'
                : 'text-muted hover:text-ink'}"
              title="Jump to this line"
              onclick={() => onseek(line.time_ms ?? 0)}
            >
              {line.text || "♪"}
            </button>
          {:else}
            <p class="min-h-3 select-text py-0.5 text-sm leading-snug text-ink/85">{line.text}</p>
          {/if}
        {/each}
      {:else if track && !loading}
        <p class="pt-2 text-sm text-muted">
          No lyrics in this file. Put a <span class="text-ink/80">.lrc</span> file with the same name next to it to
          add them.
        </p>
      {/if}
    </div>
  </div>
</aside>
