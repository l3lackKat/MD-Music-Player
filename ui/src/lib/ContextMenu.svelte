<script lang="ts" module>
  export type MenuItem =
    | {
        label: string;
        action?: () => void;
        disabled?: boolean;
        /** Shown dimmed on the right, e.g. why an item is disabled. */
        hint?: string;
        danger?: boolean;
        /** A submenu, opened on hover or with the right arrow key. */
        children?: MenuItem[];
      }
    | "separator";
</script>

<script lang="ts">
  // An app-styled menu at a screen position. Closes on a click elsewhere,
  // Escape, scrolling, resizing or the window losing focus.
  //
  // Submenus are `position: fixed` inside their parent, so the parent must
  // not get a transform, filter or backdrop-filter: any of those would make
  // it the containing block and throw the submenu off its coordinates.
  import { onMount, tick } from "svelte";
  import Self from "./ContextMenu.svelte";

  let {
    x,
    y,
    items,
    onclose,
    submenu = false,
  }: {
    x: number;
    y: number;
    items: MenuItem[];
    onclose: () => void;
    /** Set on nested menus, which leave closing to the top menu. */
    submenu?: boolean;
  } = $props();

  let menu = $state<HTMLDivElement>();
  let left = $state(0);
  let top = $state(0);
  let ready = $state(false);
  /** Index of the item whose submenu is open. */
  let openIndex = $state<number | null>(null);
  let subX = $state(0);
  let subY = $state(0);
  let itemButtons = $state<HTMLButtonElement[]>([]);
  let closeTimer: ReturnType<typeof setTimeout> | undefined;

  const MARGIN = 6;

  onMount(async () => {
    await tick();
    if (!menu) return;
    const rect = menu.getBoundingClientRect();
    const maxX = window.innerWidth - rect.width - MARGIN;
    const maxY = window.innerHeight - rect.height - MARGIN;
    // A submenu that does not fit on the right opens to the left of its parent.
    left = submenu && x > maxX ? Math.max(MARGIN, x - rect.width - subParentWidth()) : Math.min(x, maxX);
    top = Math.max(MARGIN, Math.min(y, maxY));
    ready = true;
    if (!submenu) focusItem(0);
  });

  /** Width of the parent menu, read from the item that opened this one. */
  function subParentWidth(): number {
    return (menu?.parentElement?.closest("[role=menu]") as HTMLElement | null)?.offsetWidth ?? 0;
  }

  function enabledButtons(): HTMLButtonElement[] {
    return itemButtons.filter((b) => b && !b.disabled);
  }

  function focusItem(position: number) {
    const buttons = enabledButtons();
    if (buttons.length === 0) return;
    buttons[(position + buttons.length) % buttons.length].focus();
  }

  function openSub(index: number) {
    clearTimeout(closeTimer);
    const button = itemButtons[index];
    if (!button) return;
    const rect = button.getBoundingClientRect();
    subX = rect.right - 2;
    subY = rect.top - 5;
    openIndex = index;
  }

  function choose(item: Exclude<MenuItem, "separator">, index: number) {
    if (item.disabled) return;
    if (item.children) {
      openSub(index);
      return;
    }
    onclose();
    item.action?.();
  }

  function onKeydown(event: KeyboardEvent) {
    const buttons = enabledButtons();
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "ArrowDown") focusItem(current + 1);
    else if (event.key === "ArrowUp") focusItem(current - 1);
    else if (event.key === "Home") focusItem(0);
    else if (event.key === "End") focusItem(-1);
    else if (event.key === "ArrowRight") {
      const index = itemButtons.indexOf(document.activeElement as HTMLButtonElement);
      const item = items[index];
      if (item && item !== "separator" && item.children) {
        openSub(index);
        tick().then(() => (menu?.querySelector("[role=menu] [role=menu] button:not([disabled])") as HTMLElement)?.focus());
      }
    } else if (event.key === "ArrowLeft" && submenu) {
      // Let the parent close this submenu and take focus back.
      return;
    } else if (event.key === "Escape" && !submenu) onclose();
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  function onPointerDownOutside(event: PointerEvent) {
    if (submenu) return;
    if (!menu?.contains(event.target as Node)) onclose();
  }
</script>

<svelte:window
  onpointerdown={onPointerDownOutside}
  onblur={() => !submenu && onclose()}
  onresize={() => !submenu && onclose()}
  onwheel={(e) => !submenu && !menu?.contains(e.target as Node) && onclose()}
/>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div
  bind:this={menu}
  role="menu"
  tabindex="-1"
  class="fixed z-50 min-w-52 rounded-xl border border-line bg-raised p-1 text-sm shadow-2xl shadow-black/50 {ready
    ? 'opacity-100'
    : 'opacity-0'}"
  style="left: {left}px; top: {top}px"
  onkeydown={(e) => {
    if (e.key === "ArrowLeft" && openIndex !== null) {
      const index = openIndex;
      openIndex = null;
      itemButtons[index]?.focus();
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    onKeydown(e);
  }}
  oncontextmenu={(e) => e.preventDefault()}
>
  {#each items as item, index}
    {#if item === "separator"}
      <div class="mx-2 my-1 h-px bg-line" role="separator"></div>
    {:else}
      <button
        bind:this={itemButtons[index]}
        role="menuitem"
        aria-haspopup={item.children ? "menu" : undefined}
        aria-expanded={item.children ? openIndex === index : undefined}
        disabled={item.disabled}
        class="flex w-full items-center gap-3 rounded-lg px-3 py-1.5 text-left outline-none transition-colors disabled:cursor-default disabled:text-muted/60
          {item.danger ? 'text-bad hover:bg-bad/15 focus:bg-bad/15' : 'hover:bg-line/70 focus:bg-line/70'}
          {openIndex === index ? 'bg-line/70' : ''}"
        onpointerenter={() => {
          if (item.children && !item.disabled) openSub(index);
          else closeTimer = setTimeout(() => (openIndex = null), 150);
        }}
        onclick={() => choose(item, index)}
      >
        <span class="flex-1 truncate">{item.label}</span>
        {#if item.hint}<span class="shrink-0 text-xs text-muted">{item.hint}</span>{/if}
        {#if item.children}
          <svg viewBox="0 0 24 24" class="h-3.5 w-3.5 shrink-0 text-muted" fill="none" stroke="currentColor" stroke-width="2.5">
            <path d="m9 6 6 6-6 6" />
          </svg>
        {/if}
      </button>
      {#if item.children && openIndex === index}
        <Self x={subX} y={subY} items={item.children} {onclose} submenu />
      {/if}
    {/if}
  {/each}
</div>
