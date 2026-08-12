import { describe, it, expect } from "vitest";
import { colorSwatch, isDefaultColor, type ColorKey } from "./colorDefaults";

describe("colorSwatch", () => {
  it("выбранный цвет показывается как есть в обеих темах", () => {
    expect(colorSwatch("color_bg", "#123456", false)).toBe("#123456");
    expect(colorSwatch("color_bg", "#123456", true)).toBe("#123456");
  });

  it("неустановленный фон — белый в светлой теме, а не индиго", () => {
    // The bug this replaces: every empty field fell back to a literal #6366f1,
    // so the "Background" swatch claimed indigo while the background was white.
    expect(colorSwatch("color_bg", "", false)).toBe("#ffffff");
    expect(colorSwatch("color_border", "", false)).toBe("#e2dfea");
  });

  it("неустановленный цвет следует за текущей темой", () => {
    expect(colorSwatch("color_bg", "", true)).toBe("#0f0f0f");
    expect(colorSwatch("color_text", "", true)).toBe("#f5f5f5");
  });

  it("пробелы считаются пустым значением", () => {
    expect(colorSwatch("color_bg", "   ", false)).toBe("#ffffff");
  });

  it("у каждого настраиваемого ключа есть дефолт в обеих темах", () => {
    const keys: ColorKey[] = [
      "color_accent",
      "color_accent_secondary",
      "color_bg",
      "color_bg_secondary",
      "color_bg_hover",
      "color_text",
      "color_border",
    ];
    for (const key of keys) {
      for (const dark of [false, true]) {
        expect(colorSwatch(key, "", dark), `${key}, dark=${dark}`).toMatch(/^#[0-9a-f]{6}$/);
      }
    }
  });

  it("второй акцент по умолчанию не равен первому", () => {
    // Mirrors palette_guard.rs on the frontend side: if these two ever match,
    // the .btn-primary gradient degenerates into a flat fill.
    for (const dark of [false, true]) {
      expect(colorSwatch("color_accent_secondary", "", dark)).not.toBe(
        colorSwatch("color_accent", "", dark),
      );
    }
  });
});

describe("isDefaultColor", () => {
  it("пустая строка и пробелы — дефолт, конкретный цвет — нет", () => {
    expect(isDefaultColor("")).toBe(true);
    expect(isDefaultColor("  ")).toBe(true);
    expect(isDefaultColor("#000000")).toBe(false);
  });
});
