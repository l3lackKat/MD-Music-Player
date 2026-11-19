// Dragging rows to a new place, with pointer events. HTML drag and drop does
// not work here: Tauri's file drop handler takes over the webview's drag events.

/** Pixels the pointer must travel before a press becomes a drag. */
const DRAG_THRESHOLD = 5;
/** Near this many pixels from the scroller's top or bottom edge, a drag scrolls it. */
const SCROLL_EDGE = 48;
const SCROLL_SPEED = 14;

/**
 * The new order after moving `rows` into the gap before `slot` (0 to `len`):
 * `order[new] = old`. The moved rows keep their own order and end up together.
 */
export function moveRows(len: number, rows: number[], slot: number): number[] {
  const moving = new Set(rows.filter((r) => r >= 0 && r < len));
  const rest = [...Array(len).keys()].filter((i) => !moving.has(i));
  const before = [...Array(Math.min(slot, len)).keys()].filter((i) => !moving.has(i)).length;
  const moved = [...moving].sort((a, b) => a - b);
  return [...rest.slice(0, before), ...moved, ...rest.slice(before)];
}

/** `order` changes nothing. */
export function isIdentity(order: number[]): boolean {
  return order.every((old, i) => old === i);
}

export interface DragOptions {
  /** The row elements, in list order. */
  rows: () => (HTMLElement | null | undefined)[];
  /** The element that scrolls the list, if any. */
  scroller?: () => Element | null | undefined;
  /** The rows were dropped into the gap before `slot`. */
  drop: (rows: number[], slot: number) => void;
}

/**
 * One list's drag state. Hook `down`, `move`, `up` and `cancel` to each row's
 * pointer events; read `active`, `dragging` and `line()` to draw it.
 */
export class RowDrag {
  /** A press turned into a drag. */
  active = $state(false);
  /** The rows being dragged. */
  dragging = $state<number[]>([]);
  /** The gap the rows would drop into. */
  slot = $state(0);

  #press: { rows: number[]; startY: number; pointer: number } | null = null;
  /** A drag just ended: the click the browser fires on release is not a click. */
  #swallow = false;
  #lastY = 0;
  #frame = 0;
  #options: DragOptions;

  constructor(options: DragOptions) {
    this.#options = options;
  }

  /** A row was pressed: `rows` move if the pointer then travels. */
  down(e: PointerEvent, rows: number[]) {
    if (e.button !== 0 || rows.length === 0) return;
    this.#press = { rows, startY: e.clientY, pointer: e.pointerId };
  }

  move(e: PointerEvent) {
    const press = this.#press;
    if (!press || e.pointerId !== press.pointer) return;
    this.#lastY = e.clientY;
    if (!this.active) {
      if (Math.abs(e.clientY - press.startY) < DRAG_THRESHOLD) return;
      this.active = true;
      this.dragging = press.rows;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      this.#frame = requestAnimationFrame(() => this.#scrollStep());
    }
    this.slot = this.#slotAt(e.clientY);
  }

  up(e: PointerEvent) {
    this.#end(e, true);
  }

  cancel(e: PointerEvent) {
    this.#end(e, false);
  }

  /**
   * Call first in the row's click handler: true when this click is only the
   * release of a drag, and should do nothing.
   */
  endedDrag(): boolean {
    const swallow = this.#swallow;
    this.#swallow = false;
    return swallow;
  }

  /** Stop any drag; call when the list goes away. */
  destroy() {
    cancelAnimationFrame(this.#frame);
  }

  /** Where to draw the drop line for row `index`, if anywhere. */
  line(index: number): "above" | "below" | null {
    if (!this.active) return null;
    const count = this.#options.rows().length;
    if (this.slot === index) return "above";
    if (index === count - 1 && this.slot === count) return "below";
    return null;
  }

  #end(e: PointerEvent, commit: boolean) {
    const press = this.#press;
    if (!press || e.pointerId !== press.pointer) return;
    const wasActive = this.active;
    this.#press = null;
    this.active = false;
    this.dragging = [];
    cancelAnimationFrame(this.#frame);
    if (wasActive) {
      // The release's click comes right after this event, if at all.
      this.#swallow = true;
      setTimeout(() => (this.#swallow = false), 0);
    }
    if (commit && wasActive) this.#options.drop(press.rows, this.slot);
  }

  #slotAt(y: number): number {
    const rows = this.#options.rows();
    for (let i = 0; i < rows.length; i++) {
      const rect = rows[i]?.getBoundingClientRect();
      if (rect && y < rect.top + rect.height / 2) return i;
    }
    return rows.length;
  }

  /** Scroll while the pointer is held near the scroller's edge. */
  #scrollStep() {
    if (!this.active) return;
    const scroller = this.#options.scroller?.();
    if (scroller) {
      const box = scroller.getBoundingClientRect();
      const by =
        this.#lastY < box.top + SCROLL_EDGE ? -SCROLL_SPEED : this.#lastY > box.bottom - SCROLL_EDGE ? SCROLL_SPEED : 0;
      if (by !== 0) {
        scroller.scrollBy(0, by);
        this.slot = this.#slotAt(this.#lastY);
      }
    }
    this.#frame = requestAnimationFrame(() => this.#scrollStep());
  }
}
