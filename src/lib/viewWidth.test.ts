import { describe, it, expect } from "vitest";

// The guard over how wide a screen is allowed to run.
//
// Every view sets its own ceiling, and they drifted: Settings stopped at 560px
// while Tasks took 860 and the board 1400. On a wide monitor that read as a
// broken screen — content clinging to the left with two thirds of the window
// empty — and the cause was invisible in any single file, because each number
// looked reasonable on its own.
//
// Two invariants are checked, both of which failed in that state:
//   1. a ceiling comes with centring, or the free space piles up on one side;
//   2. the ceilings of comparable screens do not diverge.

const SOURCES = import.meta.glob("/src/views/*.svelte", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// The root container of a view, by the class the markup puts on it.
const ROOTS: Record<string, string> = {
  "/src/views/Settings.svelte": "settings",
  "/src/views/Tasks.svelte": "page",
  "/src/views/Dashboard.svelte": "dash",
  "/src/views/Today.svelte": "today-view",
};

// The rule body of `.<name> {` inside the file's <style> block.
function ruleBody(src: string, cls: string): string | null {
  const at = src.indexOf(`  .${cls} {`);
  if (at < 0) return null;
  const end = src.indexOf("\n  }", at);
  return end < 0 ? null : src.slice(at, end);
}

function ceilingOf(src: string, cls: string): number | null {
  // A view may cap its children instead of itself (Today does), so the selector
  // with the ceiling is looked up in both forms.
  for (const sel of [cls, `${cls} > *`]) {
    const body = ruleBody(src, sel);
    if (!body) continue;
    const m = /max-width:\s*(\d+)px/.exec(body);
    if (m) return Number(m[1]);
  }
  return null;
}

function isCentred(src: string, cls: string): boolean {
  for (const sel of [cls, `${cls} > *`]) {
    const body = ruleBody(src, sel);
    if (!body) continue;
    if (/margin:\s*0 auto|margin-inline:\s*auto/.test(body)) return true;
  }
  return false;
}

describe("ширина экранов", () => {
  it("страж видит корневые контейнеры", () => {
    // A renamed class would otherwise leave this test checking nothing.
    for (const [path, cls] of Object.entries(ROOTS)) {
      expect(ceilingOf(SOURCES[path], cls), `${path}: .${cls}`).not.toBeNull();
    }
  });

  it("у каждого потолка есть центрирование", () => {
    const offenders: string[] = [];
    for (const [path, cls] of Object.entries(ROOTS)) {
      if (!isCentred(SOURCES[path], cls)) offenders.push(`${path}: .${cls} без margin auto`);
    }
    expect(offenders).toEqual([]);
  });

  it("потолки сопоставимых экранов не расходятся", () => {
    // Not one shared number: a board of columns legitimately wants more than a
    // settings form. But a screen half the width of its neighbour is a mistake,
    // not a decision — that is what the 560-against-860 split was.
    const values = Object.entries(ROOTS).map(([path, cls]) => ceilingOf(SOURCES[path], cls)!);
    const min = Math.min(...values);
    const max = Math.max(...values);
    expect(max / min, `потолки: ${values.join(", ")}`).toBeLessThanOrEqual(1.5);
  });
});
