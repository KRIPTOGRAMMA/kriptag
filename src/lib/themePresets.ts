// User-saved colour sets (v0.9.96).
//
// The built-in presets in Settings.svelte only carry a pair of accents. A saved
// preset stores all seven configurable colours instead: it is meant to bring
// back a whole look, and restoring the accents while leaving someone else's
// background in place would be worse than not restoring anything.
//
// Stored as a JSON string under the `custom_theme_presets` settings key — the
// same shape `app_category_rules` uses. A separate table would buy nothing: the
// list is short, read whole, and written whole.

import type { ColorKey } from "./colorDefaults";

export const PRESET_COLOR_KEYS: ColorKey[] = [
  "color_accent",
  "color_accent_secondary",
  "color_bg",
  "color_bg_secondary",
  "color_bg_hover",
  "color_bg_card",
  "color_text_secondary",
  "color_text",
  "color_border",
];

export interface ThemePreset {
  name: string;
  colors: Partial<Record<ColorKey, string>>;
}

export const MAX_PRESETS = 12;
export const MAX_NAME_LEN = 32;

// Only #rgb / #rrggbb. Anything else came from a hand-edited settings row or a
// future key this build does not know, and must not reach the DOM as a style.
function isHexColor(v: unknown): v is string {
  return typeof v === "string" && /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(v.trim());
}

// Parsing never throws: a corrupted key must leave the user with an empty list
// and a working settings screen, not a blank one. Same contract as parseRules.
export function parsePresets(json: string): ThemePreset[] {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return [];
  }
  if (!Array.isArray(raw)) return [];

  const out: ThemePreset[] = [];
  for (const item of raw) {
    if (item === null || typeof item !== "object") continue;
    const rec = item as Record<string, unknown>;
    const name = typeof rec.name === "string" ? rec.name.trim() : "";
    if (!name) continue;

    const colorsRaw = rec.colors;
    if (colorsRaw === null || typeof colorsRaw !== "object") continue;
    const src = colorsRaw as Record<string, unknown>;

    const colors: Partial<Record<ColorKey, string>> = {};
    for (const key of PRESET_COLOR_KEYS) {
      const v = src[key];
      // An empty string is meaningful — it means "this one follows the default"
      // — so it is kept, while junk is dropped.
      if (v === "") colors[key] = "";
      else if (isHexColor(v)) colors[key] = v.trim();
    }

    out.push({ name: name.slice(0, MAX_NAME_LEN), colors });
    if (out.length >= MAX_PRESETS) break;
  }
  return out;
}

export function serializePresets(presets: ThemePreset[]): string {
  return JSON.stringify(presets);
}

// Snapshots the colours currently in the settings form.
export function presetFromColors(
  name: string,
  settings: Partial<Record<ColorKey, string>>,
): ThemePreset {
  const colors: Partial<Record<ColorKey, string>> = {};
  for (const key of PRESET_COLOR_KEYS) {
    colors[key] = (settings[key] ?? "").trim();
  }
  return { name: name.trim().slice(0, MAX_NAME_LEN), colors };
}

// Adding replaces a preset of the same name rather than accumulating duplicates:
// "save" on a name already in the list reads as overwriting it. Returns the list
// unchanged when the name is blank or the list is full and the name is new.
export function addPreset(presets: ThemePreset[], preset: ThemePreset): ThemePreset[] {
  if (!preset.name) return presets;
  const at = presets.findIndex((p) => p.name === preset.name);
  if (at >= 0) {
    const next = presets.slice();
    next[at] = preset;
    return next;
  }
  if (presets.length >= MAX_PRESETS) return presets;
  return [...presets, preset];
}

export function removePreset(presets: ThemePreset[], name: string): ThemePreset[] {
  return presets.filter((p) => p.name !== name);
}
