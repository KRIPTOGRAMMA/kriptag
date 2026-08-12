import { describe, it, expect } from "vitest";
import {
  parsePresets, serializePresets, presetFromColors, addPreset, removePreset,
  MAX_PRESETS, MAX_NAME_LEN, PRESET_COLOR_KEYS, type ThemePreset,
} from "./themePresets";

describe("parsePresets", () => {
  it("читает сохранённый набор целиком", () => {
    const json = JSON.stringify([
      { name: "Ночной", colors: { color_accent: "#123456", color_bg: "#000000" } },
    ]);
    expect(parsePresets(json)).toEqual([
      { name: "Ночной", colors: { color_accent: "#123456", color_bg: "#000000" } },
    ]);
  });

  it("битый JSON даёт пустой список, а не исключение", () => {
    // The settings screen must still open after a hand-edited settings row.
    expect(parsePresets("не json")).toEqual([]);
    expect(parsePresets("")).toEqual([]);
    expect(parsePresets("{}")).toEqual([]);
    expect(parsePresets("null")).toEqual([]);
  });

  it("набор без имени отбрасывается", () => {
    const json = JSON.stringify([
      { name: "  ", colors: { color_accent: "#123456" } },
      { colors: { color_accent: "#123456" } },
      { name: "Живой", colors: {} },
    ]);
    expect(parsePresets(json).map(p => p.name)).toEqual(["Живой"]);
  });

  it("не-цвета выбрасываются, пустая строка сохраняется как «следует дефолту»", () => {
    const json = JSON.stringify([
      {
        name: "Смесь",
        colors: {
          color_accent: "javascript:alert(1)",
          color_bg: "#fff",
          color_text: "",
          color_border: 42,
        },
      },
    ]);
    expect(parsePresets(json)[0].colors).toEqual({ color_bg: "#fff", color_text: "" });
  });

  it("длинное имя обрезается, список ограничен сверху", () => {
    const long = "я".repeat(MAX_NAME_LEN + 20);
    expect(parsePresets(JSON.stringify([{ name: long, colors: {} }]))[0].name)
      .toHaveLength(MAX_NAME_LEN);

    const many = Array.from({ length: MAX_PRESETS + 5 }, (_, i) => ({ name: `n${i}`, colors: {} }));
    expect(parsePresets(JSON.stringify(many))).toHaveLength(MAX_PRESETS);
  });

  it("сериализация и разбор — круговой рейс", () => {
    const presets: ThemePreset[] = [{ name: "Рабочий", colors: { color_accent: "#abcdef" } }];
    expect(parsePresets(serializePresets(presets))).toEqual(presets);
  });
});

describe("presetFromColors", () => {
  it("снимает все настраиваемые ключи, незаданные — пустой строкой", () => {
    const preset = presetFromColors("Снимок", { color_accent: "#111111" });
    expect(preset.name).toBe("Снимок");
    expect(Object.keys(preset.colors)).toEqual([...PRESET_COLOR_KEYS]);
    expect(preset.colors.color_accent).toBe("#111111");
    expect(preset.colors.color_bg).toBe("");
  });

  it("имя обрезается по краям и по длине", () => {
    expect(presetFromColors("  Ночь  ", {}).name).toBe("Ночь");
    expect(presetFromColors("я".repeat(50), {}).name).toHaveLength(MAX_NAME_LEN);
  });
});

describe("addPreset", () => {
  const base: ThemePreset[] = [{ name: "Один", colors: { color_accent: "#111111" } }];

  it("одноимённый набор перезаписывается, а не дублируется", () => {
    const next = addPreset(base, { name: "Один", colors: { color_accent: "#222222" } });
    expect(next).toHaveLength(1);
    expect(next[0].colors.color_accent).toBe("#222222");
  });

  it("новый набор добавляется в конец", () => {
    expect(addPreset(base, { name: "Два", colors: {} }).map(p => p.name)).toEqual(["Один", "Два"]);
  });

  it("пустое имя не добавляется", () => {
    expect(addPreset(base, { name: "", colors: {} })).toEqual(base);
  });

  it("полный список не растёт, но перезапись в нём работает", () => {
    const full: ThemePreset[] = Array.from({ length: MAX_PRESETS }, (_, i) => ({ name: `n${i}`, colors: {} }));
    expect(addPreset(full, { name: "новый", colors: {} })).toHaveLength(MAX_PRESETS);
    const over = addPreset(full, { name: "n0", colors: { color_accent: "#333333" } });
    expect(over).toHaveLength(MAX_PRESETS);
    expect(over[0].colors.color_accent).toBe("#333333");
  });
});

describe("removePreset", () => {
  it("удаляет по имени и не трогает остальных", () => {
    const list: ThemePreset[] = [{ name: "a", colors: {} }, { name: "b", colors: {} }];
    expect(removePreset(list, "a").map(p => p.name)).toEqual(["b"]);
    expect(removePreset(list, "нет такого")).toHaveLength(2);
  });
});
