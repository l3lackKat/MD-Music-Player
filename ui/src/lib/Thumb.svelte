<script lang="ts">
  // A track's cover, made small: `row` for list rows, `grid` for the album
  // and artist grids. It loads only when scrolled into view, and a track
  // without a cover shows a plain note instead. Give the shape (size and
  // rounding) with `class`.
  import { thumbUrl } from "./api";

  let {
    path,
    size = "row",
    class: className = "",
  }: { path: string; size?: "row" | "grid"; class?: string } = $props();

  const src = $derived(thumbUrl(path, size));
  let failed = $state(false);
  // A new track, or the cover setting switched: try again.
  $effect(() => {
    void src;
    failed = false;
  });
</script>

<span class="grid shrink-0 place-items-center overflow-hidden bg-raised text-muted/60 {className}">
  {#if failed}
    <svg viewBox="0 0 24 24" class={size === "grid" ? "h-1/4 w-1/4" : "h-1/2 w-1/2"} fill="currentColor" aria-hidden="true">
      <path d="M10 17.5a2.5 2.5 0 1 1-2-2.45V5l10-2v11.5a2.5 2.5 0 1 1-2-2.45V7.1l-6 1.2z" />
    </svg>
  {:else}
    <img
      {src}
      alt=""
      loading="lazy"
      decoding="async"
      draggable="false"
      class="h-full w-full object-cover"
      onerror={() => (failed = true)}
    />
  {/if}
</span>
