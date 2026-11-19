// Selected rows of a track list, the way file lists work: click selects one,
// Ctrl+click adds or removes one, Shift+click selects a range from the last
// click. Rows are kept by track object, not position, so a selection
// survives moving, sorting and removing other rows; a rescan, which makes
// new objects, starts it afresh.

import { SvelteSet } from "svelte/reactivity";

export class Selection<T> {
  #items = new SvelteSet<T>();
  /** The row a Shift+click range starts from. */
  #anchor: T | null = null;

  has(item: T): boolean {
    return this.#items.has(item);
  }

  /** Positions in `list` of the selected rows, in list order. */
  rows(list: T[]): number[] {
    const out: number[] = [];
    list.forEach((item, i) => {
      if (this.#items.has(item)) out.push(i);
    });
    return out;
  }

  clear() {
    this.#items.clear();
    this.#anchor = null;
  }

  only(item: T) {
    this.#items.clear();
    this.#items.add(item);
    this.#anchor = item;
  }

  all(list: T[]) {
    this.#items.clear();
    for (const item of list) this.#items.add(item);
  }

  /** Select exactly these. */
  set(items: T[]) {
    this.#items.clear();
    for (const item of items) this.#items.add(item);
    this.#anchor = items[0] ?? null;
  }

  /** A click on `list[index]`, with the keys held. */
  click(list: T[], index: number, e: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) {
    const item = list[index];
    if (item === undefined) return;
    const anchor = this.#anchor === null ? -1 : list.indexOf(this.#anchor);
    if (e.shiftKey && anchor >= 0) {
      const [from, to] = anchor < index ? [anchor, index] : [index, anchor];
      if (!(e.ctrlKey || e.metaKey)) this.#items.clear();
      for (let i = from; i <= to; i++) this.#items.add(list[i]);
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      if (this.#items.has(item)) this.#items.delete(item);
      else this.#items.add(item);
      this.#anchor = item;
      return;
    }
    this.only(item);
  }
}
