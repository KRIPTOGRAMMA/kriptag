import { describe, it, expect } from "vitest";
import { KEYBIND_ACTIONS } from "./keybinds";

// The guard over "the number is the position in the sidebar".
//
// The digits were originally handed out in the order the screens were written,
// not the order they are shown: the notes graph arrived last and took Ctrl+6
// while sitting fourth in the sidebar, and Dashboard/Calendar/Settings were each
// one ahead of their visible position. Nothing fails when this drifts — the
// shortcuts all still work, they just stop meaning what the sidebar shows — so it
// went unnoticed until someone counted.
//
// NAV lives inside App.svelte and vitest cannot import from a .svelte file, so
// the order is parsed out of the source, the same way cssTokens.test.ts reads
// declarations it cannot import.

const APP = import.meta.glob("/src/App.svelte", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

function navActionIds(src: string): string[] {
  const start = src.indexOf("const NAV:");
  const end = src.indexOf("];", start);
  const block = src.slice(start, end);
  return [...block.matchAll(/actionId:\s*"([\w_]+)"/g)].map(m => m[1]);
}

describe("порядок разделов и цифровых хоткеев", () => {
  const src = APP["/src/App.svelte"];
  const navIds = navActionIds(src);

  it("страж действительно нашёл список разделов", () => {
    // Renaming NAV would otherwise leave this file checking an empty list.
    expect(navIds.length).toBeGreaterThan(4);
    expect(navIds).toContain("view_tasks");
    expect(navIds).toContain("view_graph");
  });

  it("цифры идут по порядку сайдбара, без пропусков", () => {
    const digitOf = (id: string) => {
      const combo = KEYBIND_ACTIONS.find(a => a.id === id)?.defaultCombo ?? "";
      const m = /^Ctrl\+Digit(\d)$/.exec(combo);
      return m ? Number(m[1]) : null;
    };

    // Only the sections that carry a digit take part: "Сегодня" is deliberately
    // outside the run on Ctrl+`.
    const numbered = navIds.map(id => ({ id, digit: digitOf(id) })).filter(x => x.digit !== null);
    const digits = numbered.map(x => x.digit);

    expect(
      digits,
      `порядок в сайдбаре: ${numbered.map(x => `${x.id}=${x.digit}`).join(", ")}`,
    ).toEqual(digits.map((_, i) => i + 1));
  });

  it("«Сегодня» не занимает цифру", () => {
    const today = KEYBIND_ACTIONS.find(a => a.id === "view_today")?.defaultCombo;
    expect(today).toBe("Ctrl+Backquote");
  });

  it("каждый раздел сайдбара есть в реестре хоткеев", () => {
    const known = new Set(KEYBIND_ACTIONS.map(a => a.id));
    expect(navIds.filter(id => !known.has(id))).toEqual([]);
  });
});
