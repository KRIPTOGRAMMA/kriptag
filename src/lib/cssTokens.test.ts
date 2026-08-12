import { describe, it, expect } from "vitest";

// The guard against referencing a design token that does not exist.
//
// `var(--text-muted, #888)` sat in five places across QuickCapture and Settings.
// No such token was ever declared, so every one of them always fell through to
// the literal grey — a colour that follows neither the theme nor the chosen
// accent. Nothing broke loudly: a fallback is exactly what makes this invisible,
// and it only became obvious once a custom background made the grey stand out.
//
// Scope: .svelte files, which Vite's ?raw returns in full. app.css itself cannot
// be read this way — for stylesheets ?raw yields an EMPTY string (see the note in
// comments.test.ts) — so the declarations are parsed out of it on the Rust side
// and mirrored here. palette_guard.rs keeps that mirror honest for the values it
// covers; this test only needs the set of names.

const SOURCES = import.meta.glob("/src/**/*.svelte", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// Every token declared in app.css :root / .dark, plus the ones components set
// on themselves as local custom properties.
const DECLARED = new Set([
  "bg-primary", "bg-secondary", "bg-card", "bg-hover",
  "text-primary", "text-secondary", "border",
  "accent", "accent-hover", "accent-secondary", "on-accent",
  "danger", "success",
  "radius", "radius-lg",
  "cat-work", "cat-study", "cat-home", "cat-health", "cat-other",
  "prio-low", "prio-medium", "prio-high", "prio-critical",
]);

// A component may define its own custom property and read it back in the same
// file; those are legitimate and are collected per file rather than globally.
function locallyDefined(src: string): Set<string> {
  const out = new Set<string>();
  for (const m of src.matchAll(/(?:^|[\s;{"'])--([\w-]+)\s*:/g)) out.add(m[1]);
  // Set from script rather than declared in CSS: `el.style.setProperty("--x", …)`.
  // A value that only exists at runtime — the board measures its own offset and
  // publishes it for the columns to size against — has no CSS declaration to find,
  // and without this the guard reports it as a reference to a missing token.
  for (const m of src.matchAll(/setProperty\(\s*["'`]--([\w-]+)["'`]/g)) out.add(m[1]);
  return out;
}

describe("токены оформления", () => {
  it("нет ссылок на несуществующие токены", () => {
    const offenders: string[] = [];

    for (const [path, src] of Object.entries(SOURCES)) {
      const local = locallyDefined(src);
      for (const m of src.matchAll(/var\(\s*--([\w-]+)(\{?)/g)) {
        const name = m[1];
        // `var(--cat-{c.category})` — Svelte interpolation: the real name is
        // assembled at runtime, so the prefix alone says nothing.
        if (m[2] === "{") continue;
        if (DECLARED.has(name) || local.has(name)) continue;
        offenders.push(`${path}: var(--${name})`);
      }
    }

    expect(offenders).toEqual([]);
  });

  it("страж видит настоящие обращения к токенам", () => {
    // Without this, a typo in the pattern above would yield an empty list and a
    // permanently green test.
    const used = new Set<string>();
    for (const src of Object.values(SOURCES)) {
      for (const m of src.matchAll(/var\(\s*--([\w-]+)/g)) used.add(m[1]);
    }
    expect(used.size).toBeGreaterThan(10);
    expect(used.has("text-secondary")).toBe(true);
  });
});
