<script lang="ts">
  // A track's cover art, with a placeholder when the track has none.
  import { coverUrl } from "./api";

  let { path, class: className = "" }: { path: string | null; class?: string } = $props();

  // The path whose image failed to load, so a new track gets a fresh try.
  let failedPath = $state<string | null>(null);
  const show = $derived(path !== null && failedPath !== path);
</script>

<div class="grid place-items-center overflow-hidden bg-raised text-line {className}">
  {#if show && path}
    <img
      src={coverUrl(path)}
      alt=""
      class="h-full w-full object-cover"
      draggable="false"
      onerror={() => (failedPath = path)}
    />
  {:else}
    <svg viewBox="0 0 24 24" class="h-2/5 w-2/5" fill="none" stroke="currentColor" stroke-width="1.5">
      <circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="2.5" />
    </svg>
  {/if}
</div>
