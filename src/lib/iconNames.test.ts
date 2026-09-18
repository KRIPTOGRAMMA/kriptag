import { describe, it, expect } from "vitest";

// The guard against <Icon name="..."> naming an icon that does not exist.
//
// Icon.svelte renders `<path d={PATHS[name] ?? ""} />`, so an unknown name is not
// an error of any kind: it draws an empty path, i.e. an invisible icon inside a
// perfectly real, clickable button. Typing is no help — the prop is a string, and
// svelte-check stays green. Found in v0.10.32: a close button was added to the
// quick-capture window with name="x", and no such icon was defined.
//
// The same shape as the missing-token guard in cssTokens.test.ts: a silent
// fallback is exactly what makes this invisible in review.

const SOURCES = import.meta.glob("/src/**/*.svelte", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const ICON_SRC = Object.entries(SOURCES).find(([p]) => p.endsWith("/Icon.svelte"))?.[1] ?? "";

// The keys of the PATHS record, read from the component itself rather than
// mirrored here: a copy would drift and start approving names that were removed.
function definedIcons(): Set<string> {
  const body = ICON_SRC.split("const PATHS")[1]?.split("};")[0] ?? "";
  const names = new Set<string>();
  for (const m of body.matchAll(/^\s*([\w-]+)\s*:/gm)) names.add(m[1]);
  return names;
}

describe("имена иконок", () => {
  it("Icon.svelte разобран и объявляет иконки", () => {
    // Without this the parse could silently yield nothing and the guard below
    // would pass for every name.
    const defined = definedIcons();
    expect(ICON_SRC).not.toBe("");
    expect(defined.size).toBeGreaterThan(20);
    expect(defined.has("timer")).toBe(true);
  });

  it("каждое <Icon name=\"...\"> есть в PATHS", () => {
    const defined = definedIcons();
    const offenders: string[] = [];

    for (const [path, src] of Object.entries(SOURCES)) {
      if (path.endsWith("/Icon.svelte")) continue;
      for (const m of src.matchAll(/<Icon\b[^>]*?\bname=(["'])([\w-]+)\1/g)) {
        const name = m[2];
        if (defined.has(name)) continue;
        offenders.push(`${path}: <Icon name="${name}">`);
      }
    }

    expect(
      offenders,
      "иконка с таким именем не объявлена в Icon.svelte. PATHS[name] ?? \"\" " +
        "нарисует пустой путь: кнопка останется кликабельной, но невидимой, " +
        "и ни типы, ни svelte-check этого не покажут",
    ).toEqual([]);
  });

  it("страж видит настоящие использования", () => {
    // A typo in the pattern above would yield an empty list and a permanently
    // green test.
    let used = 0;
    for (const [path, src] of Object.entries(SOURCES)) {
      if (path.endsWith("/Icon.svelte")) continue;
      for (const _ of src.matchAll(/<Icon\b[^>]*?\bname=(["'])([\w-]+)\1/g)) used++;
    }
    expect(used).toBeGreaterThan(20);
  });
});
