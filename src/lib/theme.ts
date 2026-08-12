// The single place a theme is applied from. The theme is stored in the DB
// (AppSettings), but so the main screen does not flash before the settings load we
// mirror the last applied value into localStorage and apply it synchronously at
// startup.

import { deriveSurfaces, onAccentText, isDarkColor } from "./surfaces";

export type ThemeMode = "light" | "dark" | "system";

export interface ThemeColors {
  color_accent: string;
  color_accent_secondary: string;
  color_bg: string;
  color_bg_secondary: string;
  color_bg_hover: string;
  color_bg_card: string;
  color_text_secondary: string;
  color_text: string;
  color_border: string;
}

const LS_MODE = "theme_mode";
const LS_COLORS = "theme_colors";

// Lightening a hex colour for --accent-hover (a custom accent has no hover of its own).
function lighten(hex: string, amount = 0.15): string {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const r = Math.min(255, Math.round(((n >> 16) & 0xff) + 255 * amount));
  const g = Math.min(255, Math.round(((n >> 8) & 0xff) + 255 * amount));
  const b = Math.min(255, Math.round((n & 0xff) + 255 * amount));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

let mql: MediaQueryList | null = null;
let mqlListener: ((e: MediaQueryListEvent) => void) | null = null;
let currentMode: ThemeMode = "system";

function applyDarkClass(mode: ThemeMode) {
  if (typeof document === "undefined") return;
  const dark = mode === "dark" || (mode === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

function applyColors(colors: Partial<ThemeColors>) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  const set = (name: string, value: string | undefined) => {
    if (value && value.trim()) root.style.setProperty(name, value.trim());
    else root.style.removeProperty(name);
  };
  set("--accent", colors.color_accent);
  if (colors.color_accent && colors.color_accent.trim()) {
    root.style.setProperty("--accent-hover", lighten(colors.color_accent));
  } else {
    root.style.removeProperty("--accent-hover");
  }
  // The second accent: empty means equal to the first (the .btn-primary gradient degenerates into a solid colour).
  set("--accent-secondary", colors.color_accent_secondary?.trim() ? colors.color_accent_secondary : colors.color_accent);
  // The text drawn ON the accent. Hardcoded #fff used to be legible only for a
  // dark accent: the pastel accents of the popular dark palettes gave white a
  // contrast around 2, so a filled button read as a smear.
  if (colors.color_accent?.trim()) {
    root.style.setProperty("--on-accent", onAccentText(colors.color_accent));
  } else {
    root.style.removeProperty("--on-accent");
  }
  set("--bg-primary", colors.color_bg);

  // The rest of the surface stack is derived from the background rather than set
  // independently. Four free numbers could not say "these sit above each other",
  // which is the only thing they mean: painting the background red used to leave
  // the task rows on the old dark --bg-card, because that one was not even
  // reachable from the settings screen. See surfaces.ts.
  const derived = colors.color_bg?.trim() ? deriveSurfaces(colors.color_bg) : null;
  if (derived) {
    root.style.setProperty("--bg-secondary", derived.bgSecondary);
    root.style.setProperty("--bg-card", derived.bgCard);
    root.style.setProperty("--bg-hover", derived.bgHover);
    root.style.setProperty("--border", derived.border);
    root.style.setProperty("--text-primary", derived.textPrimary);
    root.style.setProperty("--text-secondary", derived.textSecondary);
    // The parts the engine draws itself — the popup of a <select>, scrollbars,
    // the calendar in <input type="date"> — follow color-scheme, not our tokens.
    // A ground chosen by hand can be dark while theme_mode is still "light", and
    // then those would glow white on top of it.
    root.style.setProperty("color-scheme", isDarkColor(colors.color_bg!) ? "dark" : "light");
  } else {
    root.style.removeProperty("color-scheme");
    for (const name of ["--bg-secondary", "--bg-card", "--bg-hover", "--border", "--text-primary", "--text-secondary"]) {
      root.style.removeProperty(name);
    }
  }

  // Explicit overrides win over the derived values: someone who opened the
  // advanced block asked for exactly this colour and must get it. `override`
  // rather than `set` — an empty value here means "no override", and clearing
  // the property would wipe the derived value written just above.
  const override = (name: string, value: string | undefined) => {
    if (value && value.trim()) root.style.setProperty(name, value.trim());
    else if (!derived) root.style.removeProperty(name);
  };
  override("--bg-secondary", colors.color_bg_secondary);
  override("--bg-hover", colors.color_bg_hover);
  override("--bg-card", colors.color_bg_card);
  override("--text-secondary", colors.color_text_secondary);
  override("--text-primary", colors.color_text);
  override("--border", colors.color_border);
}

// Applies the theme and caches it in localStorage. For "system" it subscribes to
// system theme changes (reinstalling the listener so duplicates do not accumulate).
export function applyTheme(mode: ThemeMode, colors: Partial<ThemeColors>) {
  currentMode = mode;
  applyDarkClass(mode);
  applyColors(colors);

  if (typeof window !== "undefined") {
    if (!mql) mql = window.matchMedia("(prefers-color-scheme: dark)");
    if (mqlListener) mql.removeEventListener("change", mqlListener);
    if (mode === "system") {
      mqlListener = () => applyDarkClass(currentMode);
      mql.addEventListener("change", mqlListener);
    } else {
      mqlListener = null;
    }
  }

  try {
    localStorage.setItem(LS_MODE, mode);
    localStorage.setItem(LS_COLORS, JSON.stringify(colors ?? {}));
  } catch {
    // private mode or an unavailable localStorage — not critical
  }
}

// A synchronous application from the cache before the settings load from the DB (anti-flash).
export function applyCachedTheme() {
  try {
    const mode = (localStorage.getItem(LS_MODE) as ThemeMode | null) ?? "system";
    const colors = JSON.parse(localStorage.getItem(LS_COLORS) ?? "{}") as Partial<ThemeColors>;
    applyTheme(mode, colors);
  } catch {
    applyTheme("system", {});
  }
}
