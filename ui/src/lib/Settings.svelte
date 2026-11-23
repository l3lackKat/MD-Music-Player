<script lang="ts" module>
  export type SettingsTab = "output" | "playback" | "equalizer" | "appearance" | "shortcuts";
</script>

<script lang="ts">
  // Every setting in one place: the output device and mode, the software
  // volume, ReplayGain, the equalizer, how the cover looks and whether covers
  // may come from the internet. The state lives
  // in App, which applies each change; this dialog only shows it and asks.
  // Closes on Escape, the close button or a click outside.
  import { onMount } from "svelte";
  import { fade, scale } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { getVersion } from "@tauri-apps/api/app";
  import type { DeviceInfo, EqSettings, GainMode, OutputMode } from "./api";
  import EqPanel from "./EqPanel.svelte";
  import Icon, { type IconName } from "./Icon.svelte";
  import Switch from "./Switch.svelte";

  let {
    tab = $bindable(),
    devices,
    deviceId,
    mode,
    wantedMode,
    exclusiveAvailable,
    hardwareVolume,
    softVolumeEnabled,
    replayGain,
    eq = $bindable(),
    disc,
    webCovers,
    showPanel,
    signal,
    onrefreshdevices,
    ondevice,
    onmode,
    onsoftvolume,
    onreplaygain,
    ondisc,
    onwebcovers,
    onpanel,
    onclose,
    onerror,
  }: {
    tab: SettingsTab;
    devices: DeviceInfo[];
    deviceId: string | null;
    /** The mode in use. */
    mode: OutputMode;
    /** The mode the user picked; shared is used instead while exclusive is unavailable. */
    wantedMode: OutputMode;
    exclusiveAvailable: boolean;
    hardwareVolume: boolean;
    softVolumeEnabled: boolean;
    replayGain: GainMode;
    eq: EqSettings | null;
    disc: boolean;
    /** Covers from the internet for albums without one. */
    webCovers: boolean;
    showPanel: boolean;
    /** What the DAC receives right now, or `null` when nothing is playing. */
    signal: { bitPerfect: boolean; badge: string; details: string } | null;
    onrefreshdevices: () => void;
    ondevice: (id: string | null) => void;
    onmode: (mode: OutputMode) => void;
    onsoftvolume: () => void;
    onreplaygain: (mode: GainMode) => void;
    ondisc: (disc: boolean) => void;
    onwebcovers: (on: boolean) => void;
    onpanel: (show: boolean) => void;
    onclose: () => void;
    onerror: (e: unknown) => void;
  } = $props();

  const TABS: { id: SettingsTab; label: string; icon: IconName }[] = [
    { id: "output", label: "Output", icon: "speaker" },
    { id: "playback", label: "Playback", icon: "gauge" },
    { id: "equalizer", label: "Equalizer", icon: "sliders" },
    { id: "appearance", label: "Appearance", icon: "palette" },
    { id: "shortcuts", label: "Shortcuts", icon: "keyboard" },
  ];

  const MODES: { id: OutputMode; title: string; text: string }[] = [
    {
      id: "exclusive",
      title: "Exclusive",
      text: "Bit-perfect. MDMusic takes over the device, so other apps are silent while it plays.",
    },
    {
      id: "shared",
      title: "Shared",
      text: "Goes through the Windows mixer, so other apps keep playing. Windows may resample the audio.",
    },
  ];

  const GAINS: { id: GainMode; title: string; text: string }[] = [
    { id: "off", title: "Off", text: "Every track plays at your volume, as it was mastered." },
    { id: "track", title: "By track", text: "Every track plays at the same loudness. Good for mixed playlists and shuffle." },
    {
      id: "album",
      title: "By album",
      text: "Albums play at the same loudness, with their quiet and loud tracks kept as they are.",
    },
  ];

  const SHORTCUTS: [string, string][] = [
    ["Space", "Play or pause"],
    ["Ctrl + F", "Search"],
    ["Ctrl + ,", "Open settings"],
    ["Double-click / Enter", "Play the track"],
    ["Ctrl + click / Shift + click", "Select several tracks"],
    ["Ctrl + A", "Select every track in the list"],
    ["Delete", "Remove the selected tracks"],
    ["Right-click", "More actions for the selected tracks"],
    ["Drag a queue row", "Move tracks in the queue"],
    ["Esc", "Clear the selection, the search, or close a dialog"],
  ];

  let dialog = $state<HTMLDivElement>();
  let version = $state<string | null>(null);

  onMount(() => {
    dialog?.focus();
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
  });

  $effect(() => {
    if (tab === "output") onrefreshdevices();
  });

  const deviceName = $derived(devices.find((d) => d.id === deviceId)?.name ?? "System default");
</script>

{#snippet section(title: string, text?: string)}
  <h3 class="text-sm font-semibold">{title}</h3>
  {#if text}<p class="mt-0.5 text-xs leading-relaxed text-muted">{text}</p>{/if}
{/snippet}

{#snippet choice(selected: boolean, disabled: boolean, title: string, text: string, onpick: () => void)}
  <button
    role="radio"
    aria-checked={selected}
    {disabled}
    class="flex w-full items-start gap-3 rounded-xl border p-3.5 text-left transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-45 {selected
      ? 'border-accent/60 bg-accent/10'
      : 'border-line enabled:hover:border-muted/40 enabled:hover:bg-raised/60'}"
    onclick={onpick}
  >
    <span
      class="mt-0.5 grid h-4 w-4 shrink-0 place-items-center rounded-full border-2 transition-colors {selected
        ? 'border-accent'
        : 'border-muted/50'}"
      aria-hidden="true"
    >
      {#if selected}<span class="h-1.5 w-1.5 rounded-full bg-accent"></span>{/if}
    </span>
    <span class="min-w-0">
      <span class="block text-sm font-medium">{title}</span>
      <span class="mt-0.5 block text-xs leading-relaxed text-muted">{text}</span>
    </span>
  </button>
{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="fixed inset-0 z-40 grid place-items-center bg-black/55 p-6"
  transition:fade={{ duration: 150 }}
  onpointerdown={(e) => e.target === e.currentTarget && onclose()}
>
  <div
    bind:this={dialog}
    role="dialog"
    aria-modal="true"
    aria-label="Settings"
    tabindex="-1"
    class="flex h-[min(640px,100%)] w-full max-w-[52rem] overflow-hidden rounded-2xl border border-line bg-panel shadow-2xl shadow-black/60 outline-none"
    transition:scale={{ start: 0.97, duration: 180, easing: cubicOut }}
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onclose();
      }
    }}
  >
    <!-- Tabs -->
    <nav class="flex w-48 shrink-0 flex-col gap-0.5 border-r border-line bg-base/40 p-3" aria-label="Settings sections">
      <h2 class="px-3 pb-3 pt-1.5 text-lg font-semibold">Settings</h2>
      {#each TABS as item (item.id)}
        <button
          class="flex items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm transition-colors duration-150 {tab === item.id
            ? 'bg-raised font-medium text-ink'
            : 'text-muted hover:bg-raised/60 hover:text-ink'}"
          aria-current={tab === item.id ? "page" : undefined}
          onclick={() => (tab = item.id)}
        >
          <Icon name={item.icon} class="h-4 w-4 {tab === item.id ? 'text-accent' : ''}" />
          {item.label}
          {#if item.id === "equalizer" && eq?.enabled}
            <span class="ml-auto h-1.5 w-1.5 rounded-full bg-accent" title="On"></span>
          {/if}
        </button>
      {/each}
      <p class="mt-auto px-3 text-[11px] text-muted/70">MDMusic{version ? ` ${version}` : ""}</p>
    </nav>

    <div class="flex min-w-0 flex-1 flex-col">
      <header class="flex items-center justify-between px-7 pb-1 pt-5">
        <h2 class="text-[15px] font-semibold">{TABS.find((t) => t.id === tab)?.label}</h2>
        <button
          class="grid h-8 w-8 place-items-center rounded-lg text-muted transition-colors hover:bg-raised hover:text-ink"
          onclick={onclose}
          aria-label="Close settings"
          title="Close (Esc)"
        >
          <Icon name="close" class="h-4 w-4" />
        </button>
      </header>

      {#key tab}
        <div class="min-h-0 flex-1 overflow-y-auto px-7 pb-7 pt-3" in:fade={{ duration: 140 }}>
          {#if tab === "output"}
            <div class="space-y-7">
              <section>
                {@render section(
                  "Device",
                  "Where the music goes. Each device keeps its own volume. One found at 100% is turned down to 10% first, to protect your ears.",
                )}
                <div class="mt-3 flex gap-2">
                  <select
                    class="min-w-0 flex-1 truncate rounded-lg border border-line bg-raised px-3 py-2 text-sm outline-none focus:border-accent"
                    value={deviceId ?? ""}
                    aria-label="Output device"
                    onfocus={onrefreshdevices}
                    onchange={(e) => {
                      const id = e.currentTarget.value;
                      ondevice(id === "" ? null : id);
                    }}
                  >
                    <option value="">System default</option>
                    {#each devices as device (device.id)}
                      <option value={device.id}>{device.name}</option>
                    {/each}
                  </select>
                  <button
                    class="grid w-10 shrink-0 place-items-center rounded-lg border border-line text-muted transition-colors hover:bg-raised hover:text-ink"
                    title="Look for devices again"
                    aria-label="Look for devices again"
                    onclick={onrefreshdevices}
                  >
                    <Icon name="refresh" class="h-4 w-4" />
                  </button>
                </div>
              </section>

              <section>
                {@render section("Mode")}
                <div class="mt-3 grid gap-2 sm:grid-cols-2" role="radiogroup" aria-label="Output mode">
                  {#each MODES as item (item.id)}
                    {@render choice(
                      mode === item.id,
                      item.id === "exclusive" && !exclusiveAvailable,
                      item.title,
                      item.id === "exclusive" && !hardwareVolume && exclusiveAvailable
                        ? "MDMusic takes over the device. The software volume sets the level, so it is not bit-perfect."
                        : item.text,
                      () => onmode(item.id),
                    )}
                  {/each}
                </div>
                {#if wantedMode === "exclusive" && !exclusiveAvailable}
                  <p class="mt-2.5 flex items-start gap-2 rounded-lg bg-warn/10 px-3 py-2 text-xs leading-relaxed text-warn">
                    <Icon name="alert" class="mt-px h-3.5 w-3.5" />
                    <span>
                      {deviceName} has no adjustable volume of its own, so exclusive mode would play at full volume. Shared
                      mode is used instead. Turn on the software volume below, or pick a device with a hardware volume.
                    </span>
                  </p>
                {/if}
              </section>

              <section class="flex items-start gap-4 rounded-xl border border-line p-4">
                <div class="min-w-0 flex-1">
                  {@render section(
                    "Software volume",
                    "For devices without a volume of their own: lets exclusive mode work by scaling the samples. Playback is then not bit-perfect, and DSD is skipped.",
                  )}
                  <p class="mt-1.5 text-xs {hardwareVolume ? 'text-muted/80' : softVolumeEnabled ? 'text-warn' : 'text-muted'}">
                    {hardwareVolume
                      ? `${deviceName} has a hardware volume, so this is not used on it.`
                      : softVolumeEnabled
                        ? `In use on ${deviceName} in exclusive mode.`
                        : `${deviceName} has no hardware volume.`}
                  </p>
                </div>
                <Switch checked={softVolumeEnabled} label="Software volume" tone="warn" onchange={onsoftvolume} />
              </section>

              {#if signal}
                <section>
                  {@render section("Now playing")}
                  <div class="mt-3 rounded-xl border border-line bg-base/40 p-4">
                    <span
                      class="inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium {signal.bitPerfect
                        ? 'bg-good/15 text-good'
                        : 'bg-warn/15 text-warn'}"
                    >
                      {#if signal.bitPerfect}<Icon name="check" class="h-3.5 w-3.5" />{/if}
                      {signal.badge}
                    </span>
                    <p class="mt-3 select-text whitespace-pre-line text-xs leading-relaxed text-muted">{signal.details}</p>
                  </div>
                </section>
              {/if}
            </div>
          {:else if tab === "playback"}
            <section>
              {@render section(
                "ReplayGain",
                "Evens out loudness between tracks using their ReplayGain tags. It moves the device volume, never the samples, so playback stays bit-perfect. Tracks without tags play at your level.",
              )}
              <div class="mt-3 space-y-2" role="radiogroup" aria-label="ReplayGain">
                {#each GAINS as item (item.id)}
                  {@render choice(replayGain === item.id, false, item.title, item.text, () => onreplaygain(item.id))}
                {/each}
              </div>
            </section>
            <p class="mt-6 text-xs leading-relaxed text-muted">
              Pause, resume and closing the app fade the device volume smoothly, so the music never starts or stops with a
              click.
            </p>
          {:else if tab === "equalizer"}
            {#if eq}
              <EqPanel bind:settings={eq} {onerror} />
            {:else}
              <p class="text-sm text-muted">Loading the equalizer…</p>
            {/if}
          {:else if tab === "appearance"}
            <div class="space-y-7">
              <section>
                {@render section("Cover style", "You can also click any cover to switch.")}
                <div class="mt-3 grid grid-cols-2 gap-3" role="radiogroup" aria-label="Cover style">
                  {#each [{ id: false, title: "Square" }, { id: true, title: "Record" }] as item (item.title)}
                    <button
                      role="radio"
                      aria-checked={disc === item.id}
                      class="flex flex-col items-center gap-3 rounded-xl border p-4 transition-colors duration-150 {disc ===
                      item.id
                        ? 'border-accent/60 bg-accent/10'
                        : 'border-line hover:border-muted/40 hover:bg-raised/60'}"
                      onclick={() => ondisc(item.id)}
                    >
                      <span
                        class="relative grid h-20 w-20 place-items-center bg-gradient-to-br from-accent/70 to-accent/20 shadow-lg shadow-black/30 {item.id
                          ? 'rounded-full'
                          : 'rounded-lg'}"
                      >
                        {#if item.id}<span class="h-3 w-3 rounded-full bg-panel"></span>{/if}
                      </span>
                      <span class="text-sm font-medium">{item.title}</span>
                      <span class="-mt-2 text-xs text-muted">{item.id ? "Turns while the music plays" : "Album art as it is"}</span>
                    </button>
                  {/each}
                </div>
              </section>
              <section class="flex items-start gap-4 rounded-xl border border-line p-4">
                <div class="min-w-0 flex-1">
                  {@render section(
                    "Covers from the internet",
                    "For albums with no cover in their files or folder, look one up on MusicBrainz and Deezer. Only the album's title and artist are sent. Covers found are kept on this PC; your music files are never changed.",
                  )}
                </div>
                <Switch checked={webCovers} label="Covers from the internet" onchange={() => onwebcovers(!webCovers)} />
              </section>
              <section class="flex items-start gap-4 rounded-xl border border-line p-4">
                <div class="min-w-0 flex-1">
                  {@render section(
                    "Cover and lyrics panel",
                    "The panel on the right with the big cover and the lyrics. It steps aside by itself when the window is narrow.",
                  )}
                </div>
                <Switch checked={showPanel} label="Show the cover and lyrics panel" onchange={() => onpanel(!showPanel)} />
              </section>
            </div>
          {:else}
            <dl class="divide-y divide-line rounded-xl border border-line">
              {#each SHORTCUTS as [keys, what]}
                <div class="flex items-center justify-between gap-4 px-4 py-2.5 text-sm">
                  <dt class="text-muted">{what}</dt>
                  <dd class="flex shrink-0 flex-wrap justify-end gap-1">
                    {#each keys.split(" / ") as key}
                      <kbd class="rounded-md border border-line bg-raised px-2 py-0.5 font-sans text-xs text-ink">{key}</kbd>
                    {/each}
                  </dd>
                </div>
              {/each}
            </dl>
            <p class="mt-4 text-xs leading-relaxed text-muted">
              The media keys on your keyboard and the Windows media overlay control MDMusic too.
            </p>
          {/if}
        </div>
      {/key}
    </div>
  </div>
</div>
