<script lang="ts">
  // A small turning ring for work in progress. Purely a picture: put the
  // words ("Loading tracks…") next to it. `class` sets the size.
  let { class: size = "h-3 w-3" }: { class?: string } = $props();
</script>

<span
  class="inline-block shrink-0 animate-spin rounded-full border-2 border-line border-t-accent {size}"
  aria-hidden="true"
></span>
