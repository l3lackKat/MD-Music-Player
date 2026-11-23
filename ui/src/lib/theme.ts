// The colour themes. Each one is a set of the theme colours (`--color-*`) under
// `[data-theme=…]` in app.css; this module only names them, applies one to
// the page and the native window, and keeps the choice in localStorage. Both
// windows load the page, so a change in one reaches the other through the
// `storage` event.
import { getCurrentWindow } from "@tauri-apps/api/window";

export type ThemeId = "midnight" | "black" | "ocean" | "ember" | "light";

export const THEMES: { id: ThemeId; name: string; text: string; light?: boolean }[] = [
  { id: "midnight", name: "Midnight", text: "Dark grey with violet" },
  { id: "black", name: "Black", text: "True black, for OLED screens" },
  { id: "ocean", name: "Ocean", text: "Deep blue with cyan" },
  { id: "ember", name: "Ember", text: "Warm brown with orange" },
  { id: "light", name: "Light", text: "Light grey with violet", light: true },
];

const KEY = "theme";
const DEFAULT: ThemeId = "midnight";

function isTheme(id: unknown): id is ThemeId {
  return THEMES.some((t) => t.id === id);
}

export function savedTheme(): ThemeId {
  try {
    const id = localStorage.getItem(KEY);
    return isTheme(id) ? id : DEFAULT;
  } catch {
    return DEFAULT;
  }
}

/** Colours the page and the window behind it (seen while resizing), and gives
 *  the title bar the theme's light or dark look. */
export function applyTheme(id: ThemeId) {
  const root = document.documentElement;
  root.dataset.theme = id;
  const light = THEMES.find((t) => t.id === id)?.light ?? false;
  const base = getComputedStyle(root).getPropertyValue("--color-base").trim();
  const win = getCurrentWindow();
  win.setTheme(light ? "light" : "dark").catch(() => {});
  if (base) win.setBackgroundColor(base).catch(() => {});
}

export function saveTheme(id: ThemeId) {
  applyTheme(id);
  try {
    localStorage.setItem(KEY, id);
  } catch {}
}

/** Applies the saved theme now and follows changes made in the other window. */
export function startTheme() {
  applyTheme(savedTheme());
  window.addEventListener("storage", (e) => {
    if (e.key === KEY) applyTheme(savedTheme());
  });
}
