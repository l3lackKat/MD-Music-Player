<script lang="ts" module>
  /** Gains for the default 10 bands, 31 Hz to 16 kHz. */
  export const PRESETS: { name: string; gains: number[] }[] = [
    { name: "Flat", gains: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    { name: "Bass boost", gains: [6, 5, 4, 2, 0, 0, 0, 0, 0, 0] },
    { name: "Bass cut", gains: [-6, -5, -3, -1, 0, 0, 0, 0, 0, 0] },
    { name: "Treble boost", gains: [0, 0, 0, 0, 0, 0, 1, 3, 5, 6] },
    { name: "Treble cut", gains: [0, 0, 0, 0, 0, 0, -1, -3, -5, -6] },
    { name: "Loudness", gains: [5, 4, 2, 0, -1, -1, 0, 2, 4, 5] },
    { name: "Vocal", gains: [-2, -2, -1, 0, 2, 3, 3, 2, 0, -1] },
    { name: "Warm", gains: [3, 3, 2, 1, 0, 0, -1, -2, -3, -3] },
    { name: "Bright", gains: [-2, -1, 0, 0, 0, 1, 2, 3, 4, 4] },
  ];
</script>

<script lang="ts">
  // The equalizer panel. Changes are sent as they are made, one request in
  // flight at a time, and the player saves them. While the EQ is on, PCM
  // tracks are not bit-perfect; DSD always passes untouched.
  import { api, type EqBand, type EqSettings } from "./api";

  let {
    settings = $bindable(),
    onclose,
    onerror,
  }: { settings: EqSettings; onclose: () => void; onerror: (e: unknown) => void } = $props();

  const MAX_GAIN = 12;
  const PREAMP_MIN = -24;
  const PREAMP_MAX = 12;
  /** The curve is drawn for this rate; it barely differs at others below 16 kHz. */
  const CURVE_RATE = 48000;

  // ---- Sending ----
  let pending: EqSettings | null = null;
  let sending = false;

  async function send(next: EqSettings) {
    pending = next;
    if (sending) return;
    sending = true;
    try {
      while (pending !== null) {
        const value = pending;
        pending = null;
        await api.setEq($state.snapshot(value) as EqSettings);
      }
    } catch (e) {
      onerror(e);
    } finally {
      sending = false;
    }
  }

  function update(change: Partial<EqSettings>) {
    settings = { ...settings, ...change };
    void send(settings);
  }

  function setGain(index: number, gain: number) {
    update({ bands: settings.bands.map((b, i) => (i === index ? { ...b, gain_db: gain } : b)) });
  }

  // ---- Response curve ----

  /** Magnitude in dB of one peaking filter (RBJ cookbook, as in the engine) at `freq`. */
  function bandDb(band: EqBand, freq: number): number {
    if (band.gain_db === 0 || band.freq_hz >= CURVE_RATE * 0.45) return 0;
    const a = Math.pow(10, band.gain_db / 40);
    const w0 = (2 * Math.PI * band.freq_hz) / CURVE_RATE;
    const alpha = Math.sin(w0) / (2 * band.q);
    const cos = Math.cos(w0);
    const [b0, b1, b2] = [1 + alpha * a, -2 * cos, 1 - alpha * a];
    const [a0, a1, a2] = [1 + alpha / a, -2 * cos, 1 - alpha / a];
    const w = (2 * Math.PI * freq) / CURVE_RATE;
    const [c1, s1, c2, s2] = [Math.cos(w), Math.sin(w), Math.cos(2 * w), Math.sin(2 * w)];
    const numRe = b0 + b1 * c1 + b2 * c2;
    const numIm = -(b1 * s1 + b2 * s2);
    const denRe = a0 + a1 * c1 + a2 * c2;
    const denIm = -(a1 * s1 + a2 * s2);
    const mag2 = (numRe * numRe + numIm * numIm) / (denRe * denRe + denIm * denIm);
    return 10 * Math.log10(mag2);
  }

  const WIDTH = 520;
  const HEIGHT = 96;
  const RANGE_DB = 15;
  const POINTS = 160;
  const logMin = Math.log10(20);
  const logMax = Math.log10(20000);

  /** Response in dB at each plotted frequency, bands only (no preamp). */
  const response = $derived.by(() => {
    const out: number[] = [];
    for (let i = 0; i < POINTS; i++) {
      const f = Math.pow(10, logMin + ((logMax - logMin) * i) / (POINTS - 1));
      out.push(settings.bands.reduce((sum, b) => sum + bandDb(b, f), 0));
    }
    return out;
  });
  /** The highest boost of the bands: the preamp needed to never clip a full-scale tone. */
  const peakDb = $derived(Math.max(0, ...response));
  const headroomDb = $derived(peakDb + settings.preamp_db);

  const curvePath = $derived(
    response
      .map((db, i) => {
        const x = (i / (POINTS - 1)) * WIDTH;
        const total = Math.max(-RANGE_DB, Math.min(RANGE_DB, db + settings.preamp_db));
        const y = HEIGHT / 2 - (total / RANGE_DB) * (HEIGHT / 2);
        return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" "),
  );

  function xOf(freq: number): number {
    return ((Math.log10(freq) - logMin) / (logMax - logMin)) * WIDTH;
  }

  // ---- Presets ----

  const presetName = $derived(
    PRESETS.find(
      (p) => p.gains.length === settings.bands.length && p.gains.every((g, i) => g === settings.bands[i].gain_db),
    )?.name ?? "Custom",
  );

  /** Lower the preamp just enough that no frequency is boosted above full scale. */
  function autoPreamp(bands: EqBand[]): number {
    let peak = 0;
    for (let i = 0; i < POINTS; i++) {
      const f = Math.pow(10, logMin + ((logMax - logMin) * i) / (POINTS - 1));
      peak = Math.max(peak, bands.reduce((sum, b) => sum + bandDb(b, f), 0));
    }
    return -Math.ceil(peak * 2) / 2;
  }

  function applyPreset(name: string) {
    const preset = PRESETS.find((p) => p.name === name);
    if (!preset || preset.gains.length !== settings.bands.length) return;
    const bands = settings.bands.map((b, i) => ({ ...b, gain_db: preset.gains[i] }));
    update({ bands, preamp_db: autoPreamp(bands) });
  }

  function label(freq: number): string {
    return freq >= 1000 ? `${freq / 1000}k` : `${freq}`;
  }

  function signed(db: number): string {
    const v = Math.round(db * 10) / 10;
    return v > 0 ? `+${v}` : `${v}`;
  }

  /** Fill from the 0 dB line to the thumb, as percentages along the slider. */
  function fillStyle(value: number, min: number, max: number): string {
    const at = ((value - min) / (max - min)) * 100;
    const zero = ((0 - min) / (max - min)) * 100;
    return `--lo: ${Math.min(at, zero)}%; --hi: ${Math.max(at, zero)}%`;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="fixed bottom-28 right-5 z-40 w-[580px] max-w-[calc(100vw-2.5rem)] rounded-2xl border border-line bg-panel p-5 shadow-2xl shadow-black/60"
  role="dialog"
  aria-label="Equalizer"
  tabindex="-1"
  onkeydown={(e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }}
>
  <div class="flex items-center gap-3">
    <h2 class="text-[15px] font-semibold">Equalizer</h2>
    <button
      role="switch"
      aria-checked={settings.enabled}
      aria-label="Equalizer on"
      class="relative h-5 w-9 rounded-full transition-colors {settings.enabled ? 'bg-accent' : 'bg-line'}"
      onclick={() => update({ enabled: !settings.enabled })}
    >
      <span
        class="absolute top-0.5 h-4 w-4 rounded-full bg-ink transition-[left] {settings.enabled
          ? 'left-[18px]'
          : 'left-0.5'}"
      ></span>
    </button>
    <span class="text-xs {settings.enabled ? 'text-warn' : 'text-muted'}">
      {settings.enabled ? "On: PCM is no longer bit-perfect" : "Off: bit-perfect"}
    </span>

    <select
      class="ml-auto rounded-lg border border-line bg-raised px-2.5 py-1 text-sm outline-none focus:border-accent"
      value={presetName}
      onchange={(e) => applyPreset(e.currentTarget.value)}
      aria-label="Preset"
    >
      {#if presetName === "Custom"}<option value="Custom" disabled>Custom</option>{/if}
      {#each PRESETS as preset}
        <option value={preset.name}>{preset.name}</option>
      {/each}
    </select>
    <button
      class="grid h-7 w-7 place-items-center rounded-lg text-muted hover:bg-raised hover:text-ink"
      onclick={onclose}
      aria-label="Close equalizer"
    >
      ✕
    </button>
  </div>

  <!-- Response: the bands plus the preamp -->
  <svg
    viewBox="0 0 {WIDTH} {HEIGHT}"
    class="mt-4 h-24 w-full rounded-lg bg-base/60 transition-opacity {settings.enabled ? '' : 'opacity-50'}"
    preserveAspectRatio="none"
    aria-hidden="true"
  >
    {#each [100, 1000, 10000] as f}
      <line x1={xOf(f)} x2={xOf(f)} y1="0" y2={HEIGHT} class="stroke-line" stroke-width="1" />
    {/each}
    <line x1="0" x2={WIDTH} y1={HEIGHT / 2} y2={HEIGHT / 2} class="stroke-line" stroke-width="1" stroke-dasharray="3 3" />
    <path d={curvePath} fill="none" class={settings.enabled ? "stroke-accent" : "stroke-muted"} stroke-width="2" vector-effect="non-scaling-stroke" />
  </svg>

  <div class="mt-4 flex items-end gap-1">
    <!-- Preamp, set apart from the bands -->
    <div class="flex w-14 flex-col items-center gap-2 border-r border-line pr-2">
      <span class="text-[11px] tabular-nums {headroomDb > 0.05 ? 'text-warn' : 'text-muted'}">
        {signed(settings.preamp_db)}
      </span>
      <input
        class="eq-slider"
        type="range"
        min={PREAMP_MIN}
        max={PREAMP_MAX}
        step="0.5"
        value={settings.preamp_db}
        style={fillStyle(settings.preamp_db, PREAMP_MIN, PREAMP_MAX)}
        aria-label="Preamp"
        title="Preamp: double-click for 0 dB"
        oninput={(e) => update({ preamp_db: Number(e.currentTarget.value) })}
        ondblclick={() => update({ preamp_db: 0 })}
      />
      <span class="text-[11px] text-muted">Pre</span>
    </div>

    {#each settings.bands as band, index}
      <div class="flex flex-1 flex-col items-center gap-2">
        <span class="text-[11px] tabular-nums {band.gain_db === 0 ? 'text-muted' : 'text-ink'}">
          {signed(band.gain_db)}
        </span>
        <input
          class="eq-slider"
          type="range"
          min={-MAX_GAIN}
          max={MAX_GAIN}
          step="0.5"
          value={band.gain_db}
          style={fillStyle(band.gain_db, -MAX_GAIN, MAX_GAIN)}
          aria-label="{label(band.freq_hz)} Hz"
          title="{label(band.freq_hz)} Hz: double-click for 0 dB"
          oninput={(e) => setGain(index, Number(e.currentTarget.value))}
          ondblclick={() => setGain(index, 0)}
        />
        <span class="text-[11px] text-muted">{label(band.freq_hz)}</span>
      </div>
    {/each}
  </div>

  <div class="mt-4 flex items-center gap-3 text-xs">
    {#if headroomDb > 0.05}
      <span class="text-warn">Boosts exceed the preamp by {headroomDb.toFixed(1)} dB: loud passages may clip.</span>
    {:else}
      <span class="text-muted">Changes reach your ears in under a second. DSD is never equalized.</span>
    {/if}
    <span class="ml-auto flex shrink-0 gap-1">
      <button
        class="rounded-lg px-2.5 py-1 text-muted hover:bg-raised hover:text-ink disabled:opacity-40"
        disabled={Math.abs(settings.preamp_db + Math.ceil(peakDb * 2) / 2) < 0.01}
        title="Lower the preamp so no boost can clip"
        onclick={() => update({ preamp_db: autoPreamp(settings.bands) })}
      >
        Prevent clipping
      </button>
      <button
        class="rounded-lg px-2.5 py-1 text-muted hover:bg-raised hover:text-ink"
        onclick={() => update({ preamp_db: 0, bands: settings.bands.map((b) => ({ ...b, gain_db: 0 })) })}
      >
        Reset
      </button>
    </span>
  </div>
</div>
