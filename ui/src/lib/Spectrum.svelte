<script lang="ts">
  // A dialog with a whole track's spectrogram and a few plain measurements
  // (format, bits in use, cutoff). It passes no judgement: the listener
  // reads the picture. The plot is drawn at a fixed height per kHz, so it is
  // as tall as the sample rate is high, and 0–22 kHz looks the same in every
  // file. Opened from the track menu. Closing it calls the analysis off.
  import { onDestroy, onMount } from "svelte";
  import { api, formatRate, formatTime, listenAnalysis, type SpectrumAnalysis, type TrackMeta } from "./api";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";

  let { track, onclose }: { track: TrackMeta; onclose: () => void } = $props();

  let result = $state<SpectrumAnalysis | null>(null);
  let levels: Uint8Array | null = null;
  let failed = $state<string | null>(null);
  let done = $state(0);
  let dialog = $state<HTMLDivElement>();
  let canvas = $state<HTMLCanvasElement>();
  let hover = $state<{ x: number; y: number } | null>(null);
  let finished = false;
  let unlisten: (() => void) | null = null;

  onMount(() => {
    dialog?.focus();
    listenAnalysis((p) => {
      if (p.path === track.path) done = p.done;
    }).then((u) => {
      if (finished) u();
      else unlisten = u;
    });
    api
      .analyzeTrack(track.path)
      .then((r) => {
        const bytes = atob(r.pixels);
        levels = new Uint8Array(bytes.length);
        for (let i = 0; i < bytes.length; i++) levels[i] = bytes.charCodeAt(i);
        result = r;
      })
      .catch((e) => (failed = String(e)))
      .finally(() => {
        finished = true;
        unlisten?.();
      });
  });

  onDestroy(() => {
    unlisten?.();
    if (!finished) {
      finished = true;
      void api.cancelAnalysis();
    }
  });

  // ---- Colours ----

  /** Dark purple through red and orange to pale yellow, like the "inferno" map. */
  const STOPS: [number, number, number][] = [
    [0, 0, 4],
    [31, 12, 72],
    [85, 15, 109],
    [136, 34, 106],
    [186, 54, 85],
    [227, 89, 51],
    [249, 140, 10],
    [249, 201, 50],
    [252, 255, 164],
  ];

  const PALETTE = (() => {
    const lut = new Uint8ClampedArray(256 * 3);
    for (let i = 0; i < 256; i++) {
      const t = (i / 255) * (STOPS.length - 1);
      const a = Math.min(Math.floor(t), STOPS.length - 2);
      const f = t - a;
      for (let c = 0; c < 3; c++) lut[i * 3 + c] = STOPS[a][c] + (STOPS[a + 1][c] - STOPS[a][c]) * f;
    }
    return lut;
  })();

  const gradient = `linear-gradient(to right, ${STOPS.map((s) => `rgb(${s.join(",")})`).join(", ")})`;

  $effect(() => {
    if (!canvas || !result || !levels) return;
    const { width, height } = result;
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return;
    const image = context.createImageData(width, height);
    for (let i = 0; i < levels.length; i++) {
      const p = levels[i] * 3;
      image.data[i * 4] = PALETTE[p];
      image.data[i * 4 + 1] = PALETTE[p + 1];
      image.data[i * 4 + 2] = PALETTE[p + 2];
      image.data[i * 4 + 3] = 255;
    }
    context.putImageData(image, 0, 0);
  });

  // ---- Axes ----

  /** Plot height per kHz: 44.1 kHz files get ~243 px, 96 kHz ~528 px, 192 kHz ~1056 px. */
  const PX_PER_KHZ = 11;
  /** Very low rates still get a readable plot. */
  const MIN_PLOT_PX = 160;
  /** One frequency tick every this many Hz, at every rate (55 px apart). */
  const FREQ_STEP = 5000;

  const nyquist = $derived(result ? result.sample_rate / 2 : 0);
  const seconds = $derived(result ? result.width * result.seconds_per_column : 0);
  const plotPx = $derived(Math.max(MIN_PLOT_PX, Math.round((nyquist / 1000) * PX_PER_KHZ)));

  const freqTicks = $derived.by(() => {
    if (!result) return [];
    const ticks: number[] = [];
    for (let hz = 0; hz <= nyquist; hz += FREQ_STEP) ticks.push(hz);
    // Label the top too (22.05, 48, 96 kHz...), unless a tick is right there.
    if (nyquist - ticks[ticks.length - 1] >= FREQ_STEP * 0.3) ticks.push(nyquist);
    return ticks;
  });

  const timeTicks = $derived.by(() => {
    if (!result || seconds <= 0) return [];
    const steps = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1200];
    const every = steps.find((s) => seconds / s <= 8) ?? steps[steps.length - 1];
    const ticks: number[] = [];
    for (let s = 0; s <= seconds; s += every) ticks.push(s);
    return ticks;
  });

  function khz(hz: number): string {
    return String(Number((hz / 1000).toFixed(2)));
  }

  /** What is under the pointer: frequency, time and level. */
  const reading = $derived.by(() => {
    if (!hover || !result || !levels) return null;
    const col = Math.min(result.width - 1, Math.floor(hover.x * result.width));
    const row = Math.min(result.height - 1, Math.floor(hover.y * result.height));
    const level = levels[row * result.width + col];
    const db = result.db_floor + (level / 255) * (result.db_top - result.db_floor);
    return {
      hz: (1 - hover.y) * nyquist,
      ms: hover.x * seconds * 1000,
      db: level === 0 ? `≤ ${result.db_floor} dB` : `${Math.round(db)} dB`,
    };
  });

  function onPointer(event: PointerEvent) {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    hover = {
      x: Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)),
      y: Math.min(1, Math.max(0, (event.clientY - box.top) / box.height)),
    };
  }

  // ---- Measurements ----

  const facts = $derived.by(() => {
    if (!result) return [];
    const r = result;
    const channels = r.channels === 1 ? "Mono" : r.channels === 2 ? "Stereo" : `${r.channels} channels`;
    const rows: [string, string][] = [
      ["Format", [r.codec, r.declared_bits ? `${r.declared_bits}-bit` : null, formatRate(r.sample_rate), channels].filter(Boolean).join(" · ")],
    ];
    if (r.declared_bits && r.effective_bits) rows.push(["Bits in use", `${r.effective_bits} of ${r.declared_bits}`]);
    rows.push(["Cutoff", r.cutoff_hz !== null ? `${khz(r.cutoff_hz)} kHz` : "None"]);
    rows.push(["Length", formatTime(r.duration_ms)]);
    return rows;
  });
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
    aria-label="Spectrum"
    tabindex="-1"
    class="flex max-h-full w-full max-w-4xl flex-col overflow-hidden rounded-2xl border border-line bg-panel shadow-2xl shadow-black/60 outline-none"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onclose();
      }
    }}
  >
    <header class="flex items-center gap-4 border-b border-line p-5">
      <Cover path={track.path} class="h-12 w-12 shrink-0 rounded-lg" />
      <div class="min-w-0 flex-1">
        <p class="text-xs font-medium uppercase tracking-wider text-muted">Spectrum</p>
        <p class="select-text truncate text-lg font-semibold" title={track.title}>{track.title}</p>
      </div>
      <button
        class="grid h-8 w-8 shrink-0 place-items-center self-start rounded-lg text-muted hover:bg-raised hover:text-ink"
        onclick={onclose}
        aria-label="Close"
      >
        <Icon name="close" class="h-4 w-4" />
      </button>
    </header>

    <div class="min-h-0 flex-1 overflow-y-auto p-5">
      {#if failed}
        <p class="text-sm text-bad">{failed}</p>
      {:else if !result}
        <div class="grid h-[16rem] place-items-center">
          <div class="w-64 text-center">
            <p class="text-sm text-muted">Reading the whole track…</p>
            <div class="mt-3 h-1.5 overflow-hidden rounded-full bg-line">
              <div class="h-full rounded-full bg-accent transition-[width] duration-200" style:width="{Math.round(done * 100)}%"></div>
            </div>
            <p class="mt-2 text-xs tabular-nums text-muted">{done > 0 ? `${Math.round(done * 100)}%` : ""}</p>
          </div>
        </div>
      {:else}
        <!-- Above the plot, so a tall (hi-res) plot does not push them out of sight. -->
        <dl class="flex flex-wrap gap-x-8 gap-y-2 text-sm">
          {#each facts as [key, value]}
            <div>
              <dt class="text-xs text-muted">{key}</dt>
              <dd class="select-text tabular-nums">{value}</dd>
            </div>
          {/each}
        </dl>

        <!-- The plot, with the kHz axis on the left and the time axis below. -->
        <div class="mt-5 grid grid-cols-[2.75rem_minmax(0,1fr)]" style:grid-template-rows="{plotPx}px 1.5rem">
          <div class="relative text-right text-[11px] tabular-nums text-muted">
            {#each freqTicks as hz}
              <span class="absolute right-2 -translate-y-1/2" style:top="{(1 - hz / nyquist) * 100}%">{khz(hz)}</span>
            {/each}
            <span class="absolute -left-1 top-1/2 origin-center -translate-y-1/2 -rotate-90 text-[10px] uppercase tracking-wider">kHz</span>
          </div>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="relative cursor-crosshair overflow-hidden rounded-md border border-line bg-black"
            onpointermove={onPointer}
            onpointerleave={() => (hover = null)}
          >
            <canvas bind:this={canvas} class="absolute inset-0 h-full w-full"></canvas>
            {#each freqTicks as hz}
              <div class="pointer-events-none absolute inset-x-0 border-t border-white/10" style:top="{(1 - hz / nyquist) * 100}%"></div>
            {/each}
            {#if result.cutoff_hz !== null}
              <div
                class="pointer-events-none absolute inset-x-0 border-t border-dashed border-white/60"
                style:top="{(1 - result.cutoff_hz / nyquist) * 100}%"
              >
                <span class="absolute right-1.5 top-1 rounded bg-black/70 px-1.5 py-0.5 text-[11px] text-white">
                  {khz(result.cutoff_hz)} kHz
                </span>
              </div>
            {/if}
            {#if hover && reading}
              <div class="pointer-events-none absolute inset-y-0 border-l border-white/40" style:left="{hover.x * 100}%"></div>
              <div class="pointer-events-none absolute inset-x-0 border-t border-white/40" style:top="{hover.y * 100}%"></div>
              <!-- Next to the pointer, so it stays in view on a tall plot; it flips away from the edges. -->
              <span
                class="pointer-events-none absolute whitespace-nowrap rounded bg-black/75 px-1.5 py-0.5 text-[11px] tabular-nums text-white"
                style:left="{hover.x * 100}%"
                style:top="{hover.y * 100}%"
                style:transform="translate({hover.x > 0.7 ? 'calc(-100% - 8px)' : '8px'}, {hover.y > 0.9 ? 'calc(-100% - 8px)' : '8px'})"
              >
                {(reading.hz / 1000).toFixed(2)} kHz · {formatTime(reading.ms)} · {reading.db}
              </span>
            {/if}
          </div>
          <div></div>
          <div class="relative text-[11px] tabular-nums text-muted">
            {#each timeTicks as s, i}
              <span
                class="absolute top-1 {i === 0 ? '' : '-translate-x-1/2'}"
                style:left="{(s / seconds) * 100}%">{formatTime(s * 1000)}</span
              >
            {/each}
          </div>
        </div>

        <div class="mt-3 flex items-center gap-2 pl-[2.75rem] text-[11px] tabular-nums text-muted">
          <span>{result.db_floor} dB</span>
          <div class="h-2 w-40 rounded-full" style:background={gradient}></div>
          <span>{result.db_top} dB</span>
          <span class="ml-auto">Channels mixed to one</span>
        </div>
      {/if}
    </div>
  </div>
</div>
