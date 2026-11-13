<script lang="ts">
  // How a track got into a playlist: a hollow ring for one found in a
  // folder, a filled dot for one added as a single file.
  let { kind }: { kind: "folder" | "file" } = $props();
</script>

<span
  class="inline-block h-2 w-2 shrink-0 rounded-full {kind === 'file'
    ? 'bg-accent'
    : 'border-[1.5px] border-muted/70'}"
  title={kind === "file" ? "Added as a single file" : "From a playlist folder"}
  aria-label={kind === "file" ? "Single file" : "From a folder"}
  role="img"
></span>
