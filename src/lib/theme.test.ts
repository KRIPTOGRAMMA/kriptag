import { describe, it, expect, beforeEach, vi } from "vitest";
import { applyTheme, applyCachedTheme } from "./theme";

// jsdom does not implement matchMedia, and its localStorage is shadowed by Node's
// "empty" experimental global — we stub both with controllable fakes.
let systemDark = false;
const listeners = new Set<(e: MediaQueryListEvent) => void>();
const store = new Map<string, string>();

vi.stubGlobal("localStorage", {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, String(v)),
  removeItem: (k: string) => void store.delete(k),
  clear: () => store.clear(),
});

beforeEach(() => {
  systemDark = false;
  listeners.clear();
  localStorage.clear();
  document.documentElement.classList.remove("dark");
  document.documentElement.removeAttribute("style");
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() { return systemDark; },
    media: query,
    addEventListener: (_: string, cb: (e: MediaQueryListEvent) => void) => listeners.add(cb),
    removeEventListener: (_: string, cb: (e: MediaQueryListEvent) => void) => listeners.delete(cb),
  }));
});

function fireSystemThemeChange(dark: boolean) {
  systemDark = dark;
  for (const cb of [...listeners]) cb({ matches: dark } as MediaQueryListEvent);
}

describe("applyTheme", () => {
  it("dark ставит класс, light снимает", () => {
    applyTheme("dark", {});
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    applyTheme("light", {});
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("system следует за системной темой и реагирует на её смену", () => {
    systemDark = true;
    applyTheme("system", {});
    expect(document.documentElement.classList.contains("dark")).toBe(true);

    fireSystemThemeChange(false);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("не копит слушателей и отписывается при уходе с system", () => {
    applyTheme("system", {});
    applyTheme("system", {});
    expect(listeners.size).toBe(1);

    applyTheme("light", {});
    expect(listeners.size).toBe(0);
    // a change of the system theme no longer has any effect
    fireSystemThemeChange(true);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("кастомный акцент выставляет --accent и осветлённый --accent-hover", () => {
    applyTheme("light", { color_accent: "#000000" });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--accent")).toBe("#000000");
    // 0 + 255*0.15 ≈ 38 → #262626
    expect(root.style.getPropertyValue("--accent-hover")).toBe("#262626");
  });

  it("пустой цвет убирает переопределение (возврат к CSS-дефолту)", () => {
    applyTheme("light", { color_accent: "#112233" });
    applyTheme("light", { color_accent: "" });
    expect(document.documentElement.style.getPropertyValue("--accent")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--accent-hover")).toBe("");
  });

  it("фон сайдбара и фон наведения доезжают до своих переменных", () => {
    applyTheme("light", { color_bg_secondary: "#f4f2f8", color_bg_hover: "#eae7f2" });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bg-secondary")).toBe("#f4f2f8");
    expect(root.style.getPropertyValue("--bg-hover")).toBe("#eae7f2");
  });

  it("выбранный фон тянет за собой карточки — тот самый дефект со скриншота", () => {
    // A red background with black task rows: --bg-card was not configurable and
    // kept the value of the previous theme. It is derived from the background now.
    applyTheme("light", { color_bg: "#7a1520" });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bg-card")).not.toBe("");
    expect(root.style.getPropertyValue("--bg-secondary")).not.toBe("");
    expect(root.style.getPropertyValue("--border")).not.toBe("");
    // Captions too — --text-secondary was not reachable at all before.
    expect(root.style.getPropertyValue("--text-secondary")).not.toBe("");
  });

  it("карточки и подписи переопределяются точечно, соседи остаются выведенными", () => {
    // What the advanced block is for: one element takes its own colour while
    // everything else keeps following the background.
    applyTheme("light", {
      color_bg: "#7a1520",
      color_bg_card: "#00ff00",
      color_text_secondary: "#0000ff",
    });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bg-card")).toBe("#00ff00");
    expect(root.style.getPropertyValue("--text-secondary")).toBe("#0000ff");
    // Neighbours in the same layer are untouched and still follow the background.
    expect(root.style.getPropertyValue("--bg-secondary")).not.toBe("#00ff00");
    expect(root.style.getPropertyValue("--bg-hover")).not.toBe("#00ff00");
    expect(root.style.getPropertyValue("--border")).not.toBe("");
  });

  it("снятое переопределение возвращает выведенное, а не пустоту", () => {
    applyTheme("light", { color_bg: "#7a1520", color_bg_card: "#00ff00" });
    const derived = (() => {
      applyTheme("light", { color_bg: "#7a1520" });
      return document.documentElement.style.getPropertyValue("--bg-card");
    })();
    expect(derived).not.toBe("");
    expect(derived).not.toBe("#00ff00");
  });

  it("тёмный фон объявляет color-scheme: dark для нативных частей", () => {
    // The popup of a <select>, scrollbars and the calendar in <input type=date>
    // are drawn by the engine, which follows color-scheme rather than our tokens.
    // Without this they glow white on top of a dark theme.
    applyTheme("light", { color_bg: "#0a0f1e" });
    expect(document.documentElement.style.getPropertyValue("color-scheme")).toBe("dark");
  });

  it("светлый фон объявляет light, снятый — ничего", () => {
    applyTheme("dark", { color_bg: "#fdf6e3" });
    expect(document.documentElement.style.getPropertyValue("color-scheme")).toBe("light");

    applyTheme("dark", { color_bg: "" });
    expect(document.documentElement.style.getPropertyValue("color-scheme")).toBe("");
  });

  it("явно заданный цвет побеждает выведенный из фона", () => {
    applyTheme("light", { color_bg: "#7a1520", color_bg_secondary: "#00ff00" });
    expect(document.documentElement.style.getPropertyValue("--bg-secondary")).toBe("#00ff00");
  });

  it("снятый фон убирает и выведенные поверхности", () => {
    applyTheme("light", { color_bg: "#7a1520" });
    applyTheme("light", { color_bg: "" });
    const root = document.documentElement;
    for (const name of ["--bg-card", "--bg-secondary", "--border", "--text-secondary"]) {
      expect(root.style.getPropertyValue(name), name).toBe("");
    }
  });

  it("пустые фоны сайдбара и наведения возвращают CSS-дефолт", () => {
    applyTheme("light", { color_bg_secondary: "#f4f2f8", color_bg_hover: "#eae7f2" });
    applyTheme("light", { color_bg_secondary: "", color_bg_hover: "" });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bg-secondary")).toBe("");
    expect(root.style.getPropertyValue("--bg-hover")).toBe("");
  });
});

describe("applyCachedTheme", () => {
  it("восстанавливает режим и цвета из localStorage", () => {
    applyTheme("dark", { color_accent: "#ff0000" });
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("style");

    applyCachedTheme();
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(document.documentElement.style.getPropertyValue("--accent")).toBe("#ff0000");
  });

  it("битый кеш падает обратно на system без исключения", () => {
    localStorage.setItem("theme_colors", "не json");
    expect(() => applyCachedTheme()).not.toThrow();
  });

  // Why QuickCapture reads the settings from the DB instead of trusting this
  // function. The cache is written as a side effect of the main window applying a
  // theme; a quick window opened by a hotkey can run with that cache empty, and
  // then the theme it paints is not the user's — it is the system's.
  it("пустой кеш даёт не тему пользователя, а системную", () => {
    expect(localStorage.getItem("theme_mode")).toBeNull();

    // The user has dark set in the DB, but the system reports light.
    systemDark = false;
    applyCachedTheme();
    expect(document.documentElement.classList.contains("dark")).toBe(false);

    // Only the DB value gets it right, which is the call added in onMount.
    applyTheme("dark", {});
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});
