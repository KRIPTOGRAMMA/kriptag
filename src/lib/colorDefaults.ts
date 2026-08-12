// The CSS defaults behind each configurable colour (v0.9.95).
//
// A settings key holds an empty string until the user picks something, and an
// `<input type="color">` cannot show "unset" — it always displays some colour.
// Until now every empty field fell back to a single literal #6366f1, so an
// untouched "Background" swatch showed indigo while the actual background was
// white. The swatch was not just uninformative, it was wrong.
//
// These values mirror the token blocks in app.css. They are duplicated rather
// than read from the stylesheet because getComputedStyle would return the
// *current* value — including the user's own override, which is precisely what
// the placeholder must not show. palette_guard.rs keeps the two in step.

import { deriveSurfaces } from "./surfaces";

export type ColorKey =
  | "color_accent"
  | "color_accent_secondary"
  | "color_bg"
  | "color_bg_secondary"
  | "color_bg_hover"
  | "color_bg_card"
  | "color_text_secondary"
  | "color_text"
  | "color_border";

const LIGHT: Record<ColorKey, string> = {
  color_accent: "#6366f1",
  color_accent_secondary: "#a855f7",
  color_bg: "#ffffff",
  color_bg_secondary: "#f4f2f8",
  color_bg_hover: "#eae7f2",
  color_bg_card: "#ffffff",
  color_text_secondary: "#666666",
  color_text: "#1a1a1a",
  color_border: "#e2dfea",
};

const DARK: Record<ColorKey, string> = {
  color_accent: "#6366f1",
  color_accent_secondary: "#c084fc",
  color_bg: "#0f0f0f",
  color_bg_secondary: "#191722",
  color_bg_hover: "#242031",
  color_bg_card: "#1c1a24",
  color_text_secondary: "#999999",
  color_text: "#f5f5f5",
  color_border: "#2f2b3c",
};

// What the swatch shows: the user's value when set, otherwise what the screen
// actually renders. `bg` is the chosen background — when it is set, the surface
// colours come from it (see surfaces.ts) rather than from the theme token, and a
// swatch showing the token would contradict what is on screen.
export function colorSwatch(key: ColorKey, value: string, dark: boolean, bg = ""): string {
  const own = value.trim();
  if (own) return own;

  const derived = bg.trim() ? deriveSurfaces(bg) : null;
  if (derived) {
    if (key === "color_bg_secondary") return derived.bgSecondary;
    if (key === "color_bg_hover") return derived.bgHover;
    if (key === "color_bg_card") return derived.bgCard;
    if (key === "color_text_secondary") return derived.textSecondary;
    if (key === "color_border") return derived.border;
    if (key === "color_text") return derived.textPrimary;
  }
  return (dark ? DARK : LIGHT)[key];
}

// True when the field carries no explicit choice, so the UI can label it as
// following the default instead of implying the shown colour was chosen.
export function isDefaultColor(value: string): boolean {
  return value.trim() === "";
}
