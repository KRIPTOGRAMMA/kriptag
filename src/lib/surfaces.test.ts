import { describe, it, expect } from "vitest";
import { deriveSurfaces, parseHex, luminance, isDarkColor, onAccentText } from "./surfaces";

describe("deriveSurfaces", () => {
  it("не-цвет даёт null — вызывающий откатывается к дефолтам таблицы стилей", () => {
    expect(deriveSurfaces("")).toBeNull();
    expect(deriveSurfaces("не цвет")).toBeNull();
    expect(deriveSurfaces("rgb(1,2,3)")).toBeNull();
  });

  it("на тёмном фоне поверхности светлее фона", () => {
    const s = deriveSurfaces("#101010")!;
    const base = luminance(parseHex("#101010")!);
    for (const v of [s.bgSecondary, s.bgCard, s.bgHover, s.border]) {
      expect(luminance(parseHex(v)!)).toBeGreaterThan(base);
    }
  });

  it("на светлом фоне подложки темнее фона", () => {
    // The half that a plain lighten() could not do: on white there is nowhere
    // brighter to go, so the stack has to descend instead. The card is the
    // exception and is checked separately.
    const s = deriveSurfaces("#ffffff")!;
    const base = luminance(parseHex("#ffffff")!);
    for (const v of [s.bgSecondary, s.bgHover, s.border]) {
      expect(luminance(parseHex(v)!)).toBeLessThan(base);
    }
  });

  it("карточка на светлом фоне не темнее фона — она поднятая поверхность", () => {
    // The stock light theme: --bg-secondary is #f4f2f8 while --bg-card is
    // #ffffff. Sinking the card with the rest turns a white background grey.
    for (const bg of ["#ffffff", "#f4f2f8", "#fff8e1"]) {
      const s = deriveSurfaces(bg)!;
      expect(luminance(parseHex(s.bgCard)!), bg)
        .toBeGreaterThanOrEqual(luminance(parseHex(bg)!));
      expect(luminance(parseHex(s.bgCard)!), bg)
        .toBeGreaterThan(luminance(parseHex(s.bgSecondary)!));
    }
  });

  it("на тёмном фоне поверхности идут по возрастанию отступа", () => {
    // The relationship the four independent settings could not express: a card
    // sits above a panel, hover above a card, and the border further still.
    const s = deriveSurfaces("#7a1520")!;
    const base = luminance(parseHex("#7a1520")!);
    const d = (hex: string) => Math.abs(luminance(parseHex(hex)!) - base);
    expect(d(s.bgSecondary)).toBeLessThan(d(s.bgCard));
    expect(d(s.bgCard)).toBeLessThan(d(s.bgHover));
    expect(d(s.bgHover)).toBeLessThan(d(s.border));
  });

  it("текст читается на выбранном фоне", () => {
    expect(deriveSurfaces("#7a1520")!.textPrimary).toBe("#f5f5f5");
    expect(deriveSurfaces("#fff8e1")!.textPrimary).toBe("#1a1a1a");
  });

  it("насыщенный фон не уходит в серый", () => {
    // Mixing towards white/black instead of scaling channels: a red panel stays
    // recognisably red rather than drifting to pink or grey.
    const s = deriveSurfaces("#7a1520")!;
    const card = parseHex(s.bgCard)!;
    expect(card.r).toBeGreaterThan(card.g);
    expect(card.r).toBeGreaterThan(card.b);
  });

  it("чёрный и белый не выходят за границы канала", () => {
    for (const bg of ["#000000", "#ffffff"]) {
      const s = deriveSurfaces(bg)!;
      for (const v of [s.bgSecondary, s.bgCard, s.bgHover, s.border]) {
        expect(v).toMatch(/^#[0-9a-f]{6}$/);
        const { r, g, b } = parseHex(v)!;
        for (const ch of [r, g, b]) {
          expect(ch).toBeGreaterThanOrEqual(0);
          expect(ch).toBeLessThanOrEqual(255);
        }
      }
    }
  });
});

describe("parseHex", () => {
  it("читает #rgb и #rrggbb, с решёткой и без", () => {
    expect(parseHex("#fff")).toEqual({ r: 255, g: 255, b: 255 });
    expect(parseHex("102030")).toEqual({ r: 16, g: 32, b: 48 });
  });

  it("мусор даёт null", () => {
    expect(parseHex("#12")).toBeNull();
    expect(parseHex("javascript:alert(1)")).toBeNull();
  });
});

describe("isDarkColor", () => {
  it("делит по светлоте, а не по каналам", () => {
    expect(isDarkColor("#000000")).toBe(true);
    expect(isDarkColor("#7a1520")).toBe(true);
    expect(isDarkColor("#ffffff")).toBe(false);
    // Saturated yellow is light even though its blue channel is near zero.
    expect(isDarkColor("#ffcc00")).toBe(false);
  });
});

describe("onAccentText", () => {
  const contrast = (a: string, b: string) => {
    const [x, y] = [luminance(parseHex(a)!), luminance(parseHex(b)!)].sort((p, q) => q - p);
    return (x + 0.05) / (y + 0.05);
  };

  it("на каждом акценте пресетов текст читается", () => {
    // Hardcoded #fff used to be legible only for a dark accent: the pastel
    // accents of the popular dark palettes gave white around 2.0.
    const accents = [
      "#6366f1", "#ff6b35", "#88c0d0", "#bd93f9", "#7aa2f7",
      "#fabd2f", "#a7c080", "#1a6ea8", "#6f5b87", "#8a5a2b",
    ];
    for (const accent of accents) {
      expect(contrast(onAccentText(accent), accent), accent).toBeGreaterThanOrEqual(4.4);
    }
  });

  it("светлый акцент получает тёмный текст, тёмный — белый", () => {
    expect(onAccentText("#fabd2f")).toBe("#141414");
    expect(onAccentText("#1a6ea8")).toBe("#ffffff");
  });

  it("выбирается лучший из двух вариантов, а не порог на глаз", () => {
    for (const accent of ["#6366f1", "#ff6b35", "#88c0d0"]) {
      const chosen = contrast(onAccentText(accent), accent);
      const other = contrast(onAccentText(accent) === "#ffffff" ? "#141414" : "#ffffff", accent);
      expect(chosen, accent).toBeGreaterThanOrEqual(other);
    }
  });

  it("мусор даёт белый, а не падение", () => {
    expect(onAccentText("не цвет")).toBe("#ffffff");
  });
});
