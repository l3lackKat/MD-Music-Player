<script lang="ts">
  // A track's small cover for list rows. It loads only when scrolled into
  // view, and a track without a cover shows a plain note instead.
  import { thumbUrl } from "./api";

  let { path, class: className = "" }: { path: string; class?: string } = $props();

  let failed = $state(false);
  $effect(() => {
    void path;
    failed = false;
  });
</script>

<span class="grid shrink-0 place-items-center overflow-hidden rounded bg-raised text-muted/60 {className}">
  {#if failed}
    <svg viewBox="0 0 24 24" class="h-1/2 w-1/2" fill="currentColor" aria-hidden="true">
      <path d="M10 17.5a2.5 2.5 0 1 1-2-2.45V5l10-2v11.5a2.5 2.5 0 1 1-2-2.45V7.1l-6 1.2z" />
    </svg>
  {:else}
    <img
      src={thumbUrl(path)}
      alt=""
      loading="lazy"
      decoding="async"
      draggable="false"
      class="h-full w-full object-cover"
      onerror={() => (failed = true)}
    />
  {/if}
</span>
