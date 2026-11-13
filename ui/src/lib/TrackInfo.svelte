<script lang="ts">
  // A dialog with everything known about one file: where it is, its audio
  // format, every tag field and the embedded pictures. Opened from the
  // track menu. Closes on Escape, the close button or a click outside.
  import { onMount } from "svelte";
  import { api, formatRate, formatTime, type TrackDetails, type TrackMeta } from "./api";
  import Cover from "./Cover.svelte";

  let { track, onclose, onerror }: { track: TrackMeta; onclose: () => void; onerror: (e: unknown) => void } =
    $props();

  let details = $state<TrackDetails | null>(null);
  let failed = $state<string | null>(null);
  let dialog = $state<HTMLDivElement>();
  let copied = $state(false);

  onMount(() => {
    dialog?.focus();
    api
      .trackDetails(track.path)
      .then((d) => (details = d))
      .catch((e) => (failed = String(e)));
  });

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    const units = ["KB", "MB", "GB"];
    let value = bytes / 1024;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit++;
    }
    return `${value.toFixed(value < 10 ? 2 : 1)} ${units[unit]}`;
  }

  function channelName(n: number): string {
    return n === 1 ? "Mono" : n === 2 ? "Stereo" : `${n} channels`;
  }

  /** Rows for the Audio section, skipping what is unknown. */
  const audioRows = $derived.by(() => {
    if (!details) return [];
    const a = details.audio;
    const rows: [string, string][] = [];
    if (a.container) rows.push(["File type", a.container]);
    if (a.codec) rows.push(["Codec", a.codec]);
    if (a.dsd) rows.push(["Bit depth", "1-bit DSD"]);
    else if (a.bits) rows.push(["Bit depth", `${a.bits}-bit`]);
    else if (a.lossy_or_float) rows.push(["Bit depth", "Lossy or floating point"]);
    if (a.sample_rate)
      rows.push(["Sample rate", a.dsd ? `${(a.sample_rate / 1e6).toFixed(4)} MHz` : formatRate(a.sample_rate)]);
    if (a.channels) rows.push(["Channels", channelName(a.channels)]);
    if (a.duration_ms !== null) rows.push(["Duration", formatTime(a.duration_ms)]);
    if (a.bitrate_kbps) rows.push(["Bitrate", `${a.bitrate_kbps.toLocaleString()} kbps`]);
    return rows;
  });

  const fileRows = $derived.by(() => {
    if (!details) return [];
    const rows: [string, string][] = [
      ["Name", details.file_name],
      ["Folder", details.folder],
    ];
    if (details.size_bytes !== null) rows.push(["Size", formatSize(details.size_bytes)]);
    if (details.modified_ms !== null) rows.push(["Modified", new Date(details.modified_ms).toLocaleString()]);
    return rows;
  });

  async function copyPath() {
    try {
      await navigator.clipboard.writeText(track.path);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch (e) {
      onerror(e);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="fixed inset-0 z-40 grid place-items-center bg-black/60 p-6"
  onpointerdown={(e) => e.target === e.currentTarget && onclose()}
>
  <div
    bind:this={dialog}
    role="dialog"
    aria-modal="true"
    aria-label="Track info"
    tabindex="-1"
    class="flex max-h-full w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-line bg-panel shadow-2xl shadow-black/60 outline-none"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onclose();
      }
    }}
  >
    <header class="flex items-center gap-4 border-b border-line p-5">
      <Cover path={track.path} class="h-16 w-16 shrink-0 rounded-lg" />
      <div class="min-w-0 flex-1">
        <p class="select-text truncate text-lg font-semibold" title={track.title}>{track.title}</p>
        <p class="select-text truncate text-sm text-muted">
          {[track.artist, track.album].filter(Boolean).join(" · ") || "Unknown artist"}
        </p>
      </div>
      <button
        class="grid h-8 w-8 shrink-0 place-items-center self-start rounded-lg text-muted hover:bg-raised hover:text-ink"
        onclick={onclose}
        aria-label="Close"
      >
        ✕
      </button>
    </header>

    <div class="min-h-0 flex-1 overflow-y-auto px-5 pb-5">
      {#if failed}
        <p class="pt-5 text-sm text-bad">{failed}</p>
      {:else if !details}
        <p class="pt-5 text-sm text-muted">Reading the file…</p>
      {:else}
        {#snippet section(title: string, rows: [string, string][])}
          <h2 class="pb-2 pt-5 text-xs font-medium uppercase tracking-wider text-muted">{title}</h2>
          <dl class="grid grid-cols-[9rem_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-sm">
            {#each rows as [key, value]}
              <dt class="text-muted">{key}</dt>
              <dd class="select-text break-words">{value}</dd>
            {/each}
          </dl>
        {/snippet}

        {@render section("Audio", audioRows)}
        {#if details.audio.error}
          <p class="mt-2 text-sm text-bad">This file cannot be played: {details.audio.error}</p>
        {/if}

        {@render section("File", fileRows)}
        <div class="mt-3 flex gap-2 text-sm">
          <button class="rounded-lg border border-line px-3 py-1.5 hover:bg-raised" onclick={copyPath}>
            {copied ? "Copied" : "Copy path"}
          </button>
          <button
            class="rounded-lg border border-line px-3 py-1.5 hover:bg-raised"
            onclick={() => api.showInFolder(track.path).catch(onerror)}
          >
            Show in File Explorer
          </button>
        </div>

        {#if details.tags.length === 0}
          <h2 class="pb-2 pt-5 text-xs font-medium uppercase tracking-wider text-muted">Tags</h2>
          <p class="text-sm text-muted">
            {details.audio.dsd
              ? "Tags in DSF and DFF files cannot be read yet."
              : "This file has no tags. The title shown is its file name."}
          </p>
        {/if}
        {#each details.tags as tag}
          <h2 class="pb-2 pt-5 text-xs font-medium uppercase tracking-wider text-muted">
            Tags <span class="normal-case tracking-normal text-muted/70">· {tag.kind}</span>
          </h2>
          {#if tag.fields.length === 0}
            <p class="text-sm text-muted">This tag is empty.</p>
          {:else}
            <dl class="grid grid-cols-[9rem_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-sm">
              {#each tag.fields as field}
                <dt class="text-muted">{field.key}</dt>
                <!-- Long values such as lyrics scroll inside their own box. -->
                <dd class="max-h-40 select-text overflow-y-auto whitespace-pre-line break-words">{field.value}</dd>
              {/each}
            </dl>
          {/if}
        {/each}

        {#if details.pictures.length > 0}
          <h2 class="pb-2 pt-5 text-xs font-medium uppercase tracking-wider text-muted">Embedded pictures</h2>
          <ul class="space-y-1 text-sm">
            {#each details.pictures as picture}
              <li class="flex gap-4">
                <span class="w-36 shrink-0 text-muted">{picture.kind}</span>
                <span class="select-text">
                  {[
                    picture.width && picture.height ? `${picture.width} × ${picture.height}` : null,
                    picture.mime,
                    formatSize(picture.size_bytes),
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </li>
            {/each}
          </ul>
        {/if}

        <h2 class="pb-2 pt-5 text-xs font-medium uppercase tracking-wider text-muted">Lyrics</h2>
        <p class="text-sm">
          {#if details.lyrics}
            {details.lyrics.lines} {details.lyrics.lines === 1 ? "line" : "lines"},
            {details.lyrics.synced ? "timed" : "not timed"}
          {:else}
            <span class="text-muted">None</span>
          {/if}
        </p>
      {/if}
    </div>
  </div>
</div>
