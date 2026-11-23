<script lang="ts" module>
  /** Outline icons on a 24 px grid, 2 px strokes, round caps. `fill` ones are solid shapes. */
  const ICONS = {
    queue: { d: "M3 6h12M3 12h12M3 18h7M18 15V5l3 1M18 15a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 18 15" },
    album: { d: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18M12 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4" },
    artist: { d: "M12 13a4 4 0 1 0 0-8 4 4 0 0 0 0 8M4.5 20.5a7.5 7.5 0 0 1 15 0" },
    playlist: { d: "M3 6h12M3 12h12M3 18h8M19 21V12l2 1M19 21a2 2 0 1 1-2-2 2 2 0 0 1 2 2" },
    search: { d: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14M20 20l-3.5-3.5" },
    settings: {
      d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6",
    },
    plus: { d: "M12 5v14M5 12h14" },
    close: { d: "M18 6 6 18M6 6l12 12" },
    more: { d: "M5 12h.01M12 12h.01M19 12h.01", width: 3 },
    folder: { d: "M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2" },
    file: { d: "M14 3v5h5M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM12 17.5V11l3 1M12 17.5a1.5 1.5 0 1 1-1.5-1.5 1.5 1.5 0 0 1 1.5 1.5" },
    refresh: { d: "M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M3 21v-5h5" },
    pencil: { d: "M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5zM15 5l4 4" },
    trash: { d: "M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M10 11v6M14 11v6" },
    back: { d: "m15 18-6-6 6-6" },
    chevron: { d: "m9 6 6 6-6 6" },
    shuffle: { d: "M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5" },
    repeat: { d: "m17 2 4 4-4 4M3 11V10a4 4 0 0 1 4-4h14M7 22l-4-4 4-4M21 13v1a4 4 0 0 1-4 4H3" },
    mini: { d: "M21 12V7a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h6M15 14h6v5h-6z" },
    lyrics: { d: "M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1M15 5v14M6.5 9h5M6.5 12h5M6.5 15h3" },
    speaker: { d: "M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2M12 18a3 3 0 1 0 0-6 3 3 0 0 0 0 6M12 8h.01" },
    sliders: { d: "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M2 14h4M10 8h4M18 16h4" },
    gauge: { d: "M12 14l4-4M3.34 19a10 10 0 1 1 17.32 0" },
    palette: {
      d: "M12 22a10 10 0 1 1 10-10c0 2.5-2 3.5-3.5 3.5H16a2 2 0 0 0-1.5 3.3A1.9 1.9 0 0 1 12 22M7.5 10.5h.01M12 7.5h.01M16.5 10.5h.01",
    },
    keyboard: { d: "M4 6h16a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2M6 10h.01M10 10h.01M14 10h.01M18 10h.01M8 14h8" },
    alert: { d: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20M12 8v4M12 16h.01" },
    check: { d: "M20 6 9 17l-5-5" },
    play: { d: "M7 4.5v15l12.5-7.5z", fill: true },
    pause: { d: "M7 5h4v14H7zM13 5h4v14h-4z", fill: true },
    previous: { d: "M6 5h2v14H6zM20 5v14L9 12z", fill: true },
    next: { d: "M16 5h2v14h-2zM4 5v14l11-7z", fill: true },
  } satisfies Record<string, { d: string; fill?: boolean; width?: number }>;

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, class: className = "h-[18px] w-[18px]" }: { name: IconName; class?: string } = $props();
  const icon = $derived(ICONS[name] as { d: string; fill?: boolean; width?: number });
</script>

<svg
  viewBox="0 0 24 24"
  class="shrink-0 {className}"
  fill={icon.fill ? "currentColor" : "none"}
  stroke={icon.fill ? "none" : "currentColor"}
  stroke-width={icon.width ?? 2}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={icon.d} />
</svg>
