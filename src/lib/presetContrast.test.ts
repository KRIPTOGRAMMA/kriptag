import { describe, it, expect } from "vitest";
import { deriveSurfaces, luminance, parseHex, onAccentText } from "./surfaces";

// The guard over the built-in presets.
//
// A preset is three hex values typed by hand, and nothing about the way they are
// written says whether the result is legible. The originals these are taken from
// (Nord, Dracula, Solarized…) were tuned for syntax highlighting on one flat
// colour — not for an interface that derives a stack of surfaces from the
// background. Solarized's own #268bd2 lands at 3.56 on a card here, below
// legibility, purely because a card sits lighter than the ground it came from.
//
// So every pair is measured against the surfaces this app actually builds. The
// list is parsed out of Settings.svelte rather than duplicated: a copy would let
// a new preset be added without ever reaching this test.

const SOURCE = Object.values(
  import.meta.glob("/src/views/Settings.svelte", {
    query: "?raw",
    import: "default",
    eager: true,
  }) as Record<string, string>,
)[0];

interface Preset {
  name: string;
  accent: string;
  accentSecondary: string;
  bg?: string;
}

function parsePresets(src: string): Preset[] {
  const block = src.slice(src.indexOf("const THEME_PRESETS"));
  const end = block.indexOf("]);");
  const body = block.slice(0, end);

  const out: Preset[] = [];
  const row = /\{\s*name:\s*(?:t\()?"([^"]+)"\)?,\s*accent:\s*"([^"]+)",\s*accentSecondary:\s*"([^"]+)"(?:,\s*bg:\s*"([^"]+)")?\s*\}/g;
  for (const m of body.matchAll(row)) {
    out.push({ name: m[1], accent: m[2], accentSecondary: m[3], bg: m[4] });
  }
  return out;
}

const PRESETS = parsePresets(SOURCE);

function contrast(a: string, b: string): number {
  const [x, y] = [luminance(parseHex(a)!), luminance(parseHex(b)!)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

// WCAG AA for interface elements and large text.
const MIN = 3.0;
// The same for an accent that has to read as a small label on a surface.
const MIN_TEXT = 4.5;

describe("пресеты тем", () => {
  it("страж видит список пресетов", () => {
    // A broken pattern would silently check nothing.
    expect(PRESETS.length).toBeGreaterThanOrEqual(7);
    expect(PRESETS.map(p => p.name)).toContain("Ember");
  });

  it("акцент читается и на фоне, и на карточке", () => {
    const bad: string[] = [];
    for (const p of PRESETS) {
      if (!p.bg) continue;
      const s = deriveSurfaces(p.bg)!;
      const onBg = contrast(p.accent, p.bg);
      const onCard = contrast(p.accent, s.bgCard);
      if (onBg < MIN_TEXT) bad.push(`${p.name}: акцент на фоне ${onBg.toFixed(2)}`);
      if (onCard < MIN_TEXT) bad.push(`${p.name}: акцент на карточке ${onCard.toFixed(2)}`);
    }
    expect(bad).toEqual([]);
  });

  it("второй акцент отличим от первого и виден на фоне", () => {
    const bad: string[] = [];
    for (const p of PRESETS) {
      // A degenerate pair turns the .btn-primary gradient into a flat fill.
      if (p.accent === p.accentSecondary) bad.push(`${p.name}: доп. акцент равен основному`);
      if (!p.bg) continue;
      const onBg = contrast(p.accentSecondary, p.bg);
      if (onBg < MIN) bad.push(`${p.name}: доп. акцент на фоне ${onBg.toFixed(2)}`);
    }
    expect(bad).toEqual([]);
  });

  it("текст на заливке акцента читается", () => {
    const bad: string[] = [];
    for (const p of PRESETS) {
      const c = contrast(onAccentText(p.accent), p.accent);
      if (c < 4.4) bad.push(`${p.name}: текст на акценте ${c.toFixed(2)}`);
    }
    expect(bad).toEqual([]);
  });

  it("подписи читаются на выведенных поверхностях", () => {
    const bad: string[] = [];
    for (const p of PRESETS) {
      if (!p.bg) continue;
      const s = deriveSurfaces(p.bg)!;
      const c = contrast(s.textSecondary, s.bgCard);
      if (c < MIN) bad.push(`${p.name}: подписи на карточке ${c.toFixed(2)}`);
    }
    expect(bad).toEqual([]);
  });
});
