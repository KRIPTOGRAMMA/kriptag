import { describe, it, expect } from "vitest";

// The guard over the accent thread on the sidebar's edge.
//
// It is the one place where both accents are shown together along their full
// length — a swatch or a 90px button gradient never shows the transition itself.
// Two earlier attempts put it under the title bar and had to be removed: that
// bar is transparent, floats over the content with pointer-events:none and takes
// no height, so a thread at top:0 sat on the window frame and one at top:32px
// crossed the section heading. The sidebar's right edge is the only boundary in
// this window that actually exists.
//
// The failure mode this catches is quiet: someone restyling the sidebar writes
// `border-right: 1px solid var(--border)` back, the thread disappears, and
// nothing looks broken — just a little duller.

const SRC = Object.values(
  import.meta.glob("/src/App.svelte", {
    query: "?raw",
    import: "default",
    eager: true,
  }) as Record<string, string>,
)[0];

function ruleBody(selector: string): string | null {
  const at = SRC.indexOf(`  ${selector} {`);
  if (at < 0) return null;
  const end = SRC.indexOf("\n  }", at);
  return end < 0 ? null : SRC.slice(at, end);
}

describe("акцентная нить", () => {
  it("страж читает стили сайдбара", () => {
    // A renamed class would otherwise leave every assertion below vacuous.
    expect(ruleBody(".sidebar"), ".sidebar не найден").not.toBeNull();
  });

  it("нить нарисована на краю сайдбара обоими акцентами", () => {
    const thread = ruleBody(".sidebar::after");
    expect(thread, ".sidebar::after не найден").not.toBeNull();
    expect(thread).toContain("var(--accent)");
    expect(thread).toContain("var(--accent-secondary)");
    // Vertical: the sidebar runs the full height of the window, so the gradient
    // has room to show the transition rather than hinting at it.
    expect(thread).toContain("180deg");
    expect(thread).toContain("right: 0");
  });

  it("край сайдбара не рисуется обычной границей", () => {
    // A border cannot carry a gradient, so its return means the thread is gone.
    const sidebar = ruleBody(".sidebar")!;
    expect(sidebar).not.toMatch(/^\s*border-right:/m);
    // The pseudo-element needs a positioned parent, or it would anchor to the page.
    expect(sidebar).toContain("position: relative");
  });
});
