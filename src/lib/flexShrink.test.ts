import { describe, it, expect } from "vitest";

// The guard over "a scrolling column may shrink".
//
// A flex item defaults to min-height: auto and refuses to shrink below its
// content. Chromium happens to bound these boxes anyway because of the
// overflow: hidden on them, so a long note scrolled correctly there and the e2e
// suite never saw a problem — but the app runs in WebKitGTK, where the editor
// host grew to the full height of the note instead. CodeMirror sizes its
// scroller to that host, so the scroller had nothing left to scroll and the rest
// of the note was clipped away, unreachable.
//
// This cannot be an e2e test: Playwright drives headless Chromium, the one engine
// where the bug does not reproduce. So the rule is checked in the source instead —
// every element on the path from the notes pane down to the editor must state
// min-height: 0 rather than depend on an engine to imply it.

const SOURCES = import.meta.glob("/src/**/*.svelte", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// Selectors that carry the editor's scroll chain, by the file they live in.
const SCROLL_CHAIN: Record<string, string[]> = {
  "/src/views/Notes.svelte": ["editor-pane", "editor-body"],
  "/src/lib/components/LiveMarkdownEditor.svelte": ["cm-host"],
};

function ruleBody(src: string, cls: string): string | null {
  const at = src.indexOf(`  .${cls} {`);
  if (at < 0) return null;
  const end = src.indexOf("\n  }", at);
  return end < 0 ? null : src.slice(at, end);
}

describe("прокрутка редактора заметок", () => {
  it("страж видит все правила цепочки", () => {
    // A renamed class would otherwise leave this test checking nothing.
    for (const [path, classes] of Object.entries(SCROLL_CHAIN)) {
      for (const cls of classes) {
        expect(ruleBody(SOURCES[path], cls), `${path}: .${cls}`).not.toBeNull();
      }
    }
  });

  it("каждый flex-элемент цепочки может сжиматься", () => {
    const offenders: string[] = [];
    for (const [path, classes] of Object.entries(SCROLL_CHAIN)) {
      for (const cls of classes) {
        const body = ruleBody(SOURCES[path], cls)!;
        // Only flex items are at risk — a block box shrinks on its own.
        if (!/flex:\s*1/.test(body)) continue;
        if (!/min-height:\s*0/.test(body)) offenders.push(`${path}: .${cls} без min-height: 0`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
