import { describe, it, expect } from "vitest";

// The guard against the Settings hint naming a system idle source it cannot know.
//
// Found by the help revision (v0.10.34): the hint said "system idle/return from
// the compositor (ext-idle-notify)" with the source hardcoded. That was true
// while Wayland was the only source; the moment Windows (GetLastInputInfo,
// v0.10.30) and X11 (MIT-SCREEN-SAVER, v0.10.31) landed, the same sentence became
// wrong on two platforms out of three — and nothing failed, because a string is
// a string.
//
// The source's name now comes from the backend (get_tracking_source), so no
// source may be spelled out in the frontend at all.

const SOURCES = import.meta.glob("/src/**/*.{svelte,ts}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// The three real sources. A frontend file naming any of them is claiming to know
// which platform it is running on.
const SOURCE_NAMES = ["ext-idle-notify", "MIT-SCREEN-SAVER", "GetLastInputInfo"];

describe("подсказка о режиме трекинга", () => {
  it("ни один источник простоя не зашит во фронтенде", () => {
    const offenders: string[] = [];

    for (const [path, src] of Object.entries(SOURCES)) {
      // This test names them on purpose.
      if (path.endsWith("/trackingHint.test.ts")) continue;
      // Comments explaining WHY no source may be named are allowed — they are
      // not shown to anyone. Strip them before looking.
      const code = src
        .replace(/<!--[\s\S]*?-->/g, "")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/^\s*\/\/.*$/gm, "");

      for (const name of SOURCE_NAMES) {
        if (code.includes(name)) offenders.push(`${path}: ${name}`);
      }
    }

    expect(
      offenders,
      "имя источника обязано приходить из бэкенда (api.getTrackingSource), " +
        "иначе подсказка снова начнёт врать на платформах, где источник другой",
    ).toEqual([]);
  });

  it("подсказка действительно спрашивает источник у бэкенда", () => {
    // Without this the guard above would pass on a UI that dropped the source
    // entirely — the hint would stop lying by saying nothing at all.
    const settings = Object.entries(SOURCES).find(([p]) => p.endsWith("/Settings.svelte"))?.[1] ?? "";
    expect(settings).not.toBe("");
    expect(settings).toContain("getTrackingSource");
    expect(settings).toContain("trackingSource");
  });
});
