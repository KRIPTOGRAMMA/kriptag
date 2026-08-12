import { describe, it, expect } from "vitest";
import { formatMinutes } from "./duration";

// A stand-in for t(): substitutes {vars} into the key, which is exactly what the
// real one does when a key is missing from the dictionary.
const t = (key: string, vars: Record<string, string | number> = {}) =>
  key.replace(/\{(\w+)\}/g, (_, k) => String(vars[k] ?? ""));

const fmt = (mins: number) => formatMinutes(mins, t);

describe("formatMinutes", () => {
  it("меньше часа остаётся в минутах", () => {
    expect(fmt(0)).toBe("0 мин");
    expect(fmt(45)).toBe("45 мин");
    expect(fmt(59)).toBe("59 мин");
  });

  // The case that prompted this: a working day used to render as "384 мин".
  it("часы отделяются от минут", () => {
    expect(fmt(60)).toBe("1 ч");
    expect(fmt(90)).toBe("1 ч 30 мин");
    expect(fmt(384)).toBe("6 ч 24 мин");
  });

  it("ровные часы не тянут за собой «0 мин»", () => {
    expect(fmt(120)).toBe("2 ч");
    expect(fmt(600)).toBe("10 ч");
  });

  it("дробные минуты округляются, отрицательные зажимаются нулём", () => {
    expect(fmt(59.6)).toBe("1 ч");
    expect(fmt(-5)).toBe("0 мин");
  });
});
