<script lang="ts">
  // The Windows volume of the selected output device. It sets the device's
  // own level, the one in the Windows volume mixer, so the samples sent to
  // the DAC stay untouched and bit-perfect playback is kept.
  import { onMount } from "svelte";
  import { api, type DeviceVolume } from "./api";

  let { exclusive, onerror }: { exclusive: boolean; onerror: (e: unknown) => void } = $props();

  let volume = $state<DeviceVolume | null>(null);
  /** The level under the thumb while it is held; the device's answers do not move it. */
  let dragLevel = $state<number | null>(null);

  const level = $derived(dragLevel ?? volume?.level ?? 0);
  const percent = $derived(Math.round(level * 100));
  /** In exclusive mode Windows' own scaling is bypassed, so only a hardware control works. */
  const noEffect = $derived(exclusive && volume !== null && !volume.hardware);

  /** Read the device's volume again, e.g. after switching devices. */
  export async function refresh(): Promise<DeviceVolume | null> {
    if (dragLevel !== null) return volume;
    try {
      volume = await api.getVolume();
    } catch {
      // The device is gone or has no volume control: hide the level, no error popup.
      volume = null;
    }
    return volume;
  }

  onMount(() => void refresh());

  // Dragging fires many inputs: keep one request in flight and send only the latest level.
  let pending: number | null = null;
  let sending = false;

  async function send(target: number) {
    pending = target;
    if (sending) return;
    sending = true;
    try {
      while (pending !== null) {
        const next = pending;
        pending = null;
        // Like the Windows slider, moving the level unmutes.
        volume = await api.setVolume({ level: next, muted: volume?.muted ? false : undefined });
      }
    } catch (e) {
      onerror(e);
    } finally {
      sending = false;
    }
  }

  function onInput(value: number) {
    dragLevel = value;
    void send(value);
  }

  function onRelease(value: number) {
    if (volume) volume = { ...volume, level: value, muted: false };
    dragLevel = null;
  }

  async function toggleMute() {
    if (!volume) return;
    try {
      volume = await api.setVolume({ muted: !volume.muted });
    } catch (e) {
      onerror(e);
    }
  }

  const title = $derived(
    volume === null
      ? "This device's volume cannot be read"
      : noEffect
        ? "This device has no hardware volume, so Windows' level has no effect in exclusive mode. Use the DAC's or amplifier's own knob, or switch to shared mode."
        : `Windows volume of this device: ${percent}%${volume.muted ? " (muted)" : ""}`,
  );
</script>

<div class="flex shrink-0 items-center gap-1.5" {title}>
  <button
    class="relative grid h-8 w-8 place-items-center rounded-lg hover:bg-raised disabled:opacity-40 {volume?.muted
      ? 'text-bad'
      : 'text-muted hover:text-ink'}"
    onclick={toggleMute}
    disabled={volume === null}
    aria-pressed={volume?.muted ?? false}
    aria-label={volume?.muted ? "Unmute" : "Mute"}
  >
    <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" />
      {#if volume?.muted || volume === null}
        <path d="m16 9 5 6M21 9l-5 6" />
      {:else}
        {#if level > 0}<path d="M15.5 9.5a3.5 3.5 0 0 1 0 5" />{/if}
        {#if level >= 0.5}<path d="M18.5 6.5a8 8 0 0 1 0 11" />{/if}
      {/if}
    </svg>
    {#if noEffect}<span class="absolute right-1 top-1 h-1.5 w-1.5 rounded-full bg-warn"></span>{/if}
  </button>
  <input
    class="seek w-24"
    type="range"
    min="0"
    max="1"
    step="0.01"
    value={level}
    style="--fill: {volume?.muted ? 0 : percent}%"
    disabled={volume === null}
    aria-label="Volume"
    oninput={(e) => onInput(Number(e.currentTarget.value))}
    onchange={(e) => onRelease(Number(e.currentTarget.value))}
  />
  <span class="w-8 text-right text-xs tabular-nums {noEffect ? 'text-warn' : 'text-muted'}">
    {volume === null ? "–" : percent}
  </span>
</div>
