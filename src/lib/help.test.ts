import { describe, it, expect } from "vitest";
import { HELP_TOPICS } from "./help";

describe("HELP_TOPICS", () => {
  it("id тем уникальны — используются как ключи {#each}", () => {
    const ids = HELP_TOPICS.map(t => t.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("все темы и пункты непусты", () => {
    expect(HELP_TOPICS.length).toBeGreaterThan(0);
    for (const topic of HELP_TOPICS) {
      expect(topic.title.trim()).not.toBe("");
      expect(topic.items.length).toBeGreaterThan(0);
      for (const item of topic.items) {
        expect(item.term.trim()).not.toBe("");
        expect(item.desc.trim()).not.toBe("");
      }
    }
  });

  // The help's main risk is going stale silently. Rebindable hotkeys live as data in
  // keybinds.ts and are rendered on the "Hotkeys" tab with their CURRENT
  // combinations; duplicating them here as text would start lying at the first
  // rebinding. So the help says WHERE to look rather than which they are.
  //
  // Combinations inside input fields (Ctrl+Enter, Shift+Enter, Ctrl+V, Ctrl+Tab,
  // Ctrl+click) are a different matter: they are hardcoded in handlers, absent from
  // keybinds.ts and cannot be rebound, so they cannot go stale.
  it("не дублирует переназначаемые хоткеи — их значения только в keybinds.ts", () => {
    const text = HELP_TOPICS
      .flatMap(t => t.items.map(i => `${i.term} ${i.desc}`))
      .join(" ");
    // Navigation and palette combinations (Ctrl+K, Ctrl+D, Ctrl+1..7)
    expect(text).not.toMatch(/Ctrl\s*\+\s*[KDkd]\b/);
    expect(text).not.toMatch(/Ctrl\s*\+\s*\d/);
    // The global quick-capture hotkeys (Ctrl+Shift+N/M/B)
    expect(text).not.toMatch(/Ctrl\s*\+?\s*Shift\s*\+?\s*[NMBnmb]\b/);
  });

  // Paths depend on the OS and on the application's identifier — a hardcoded
  // `~/.local/share/kriptag/...` turned out to be wrong on every OS.
  it("не содержит захардкоженных путей", () => {
    const text = HELP_TOPICS
      .flatMap(t => t.items.map(i => i.desc))
      .join(" ");
    expect(text).not.toContain(".local/share");
    expect(text).not.toContain("%APPDATA%");
    expect(text).not.toContain("Library/Application Support");
  });
});
