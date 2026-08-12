import { describe, it, expect } from "vitest";
import {
  KEYBIND_ACTIONS,
  defaultKeybinds,
  parseKeybinds,
  comboFor,
  comboFromEvent,
  comboMatches,
  formatCombo,
  findConflicts,
} from "./keybinds";

describe("defaultKeybinds", () => {
  it("содержит запись для каждого действия из KEYBIND_ACTIONS", () => {
    const binds = defaultKeybinds();
    for (const a of KEYBIND_ACTIONS) {
      expect(binds[a.id]).toBe(a.defaultCombo);
    }
  });
});

describe("parseKeybinds", () => {
  it("валидный JSON-объект возвращается как есть", () => {
    expect(parseKeybinds('{"palette":"Ctrl+KeyJ"}')).toEqual({ palette: "Ctrl+KeyJ" });
  });

  it("пустая строка/невалидный JSON/массив — пустой объект (дефолты применятся через comboFor)", () => {
    expect(parseKeybinds("")).toEqual({});
    expect(parseKeybinds("не json")).toEqual({});
    expect(parseKeybinds("[1,2]")).toEqual({});
  });
});

describe("comboFor", () => {
  it("без оверрайда — дефолтная комбинация действия", () => {
    expect(comboFor({}, "palette")).toBe("Ctrl+KeyK");
  });

  it("с оверрайдом — сохранённое значение", () => {
    expect(comboFor({ palette: "Ctrl+KeyJ" }, "palette")).toBe("Ctrl+KeyJ");
  });

  it("неизвестное действие без оверрайда — пустая строка", () => {
    expect(comboFor({}, "unknown")).toBe("");
  });
});

describe("comboFromEvent", () => {
  it("собирает Ctrl+Shift+код в фиксированном порядке модификаторов", () => {
    expect(comboFromEvent({ ctrlKey: true, shiftKey: true, altKey: false, code: "KeyN" })).toBe("Ctrl+Shift+KeyN");
  });

  it("без модификаторов — просто код", () => {
    expect(comboFromEvent({ ctrlKey: false, shiftKey: false, altKey: false, code: "Digit1" })).toBe("Digit1");
  });

  it("нажатие только модификатора — null (ждём основную клавишу)", () => {
    expect(comboFromEvent({ ctrlKey: true, shiftKey: false, altKey: false, code: "ControlLeft" })).toBeNull();
  });
});

describe("comboMatches", () => {
  it("совпадающее событие — true", () => {
    expect(comboMatches("Ctrl+KeyK", { ctrlKey: true, shiftKey: false, altKey: false, code: "KeyK" })).toBe(true);
  });

  it("несовпадающие модификаторы — false", () => {
    expect(comboMatches("Ctrl+KeyK", { ctrlKey: true, shiftKey: true, altKey: false, code: "KeyK" })).toBe(false);
  });
});

describe("formatCombo", () => {
  it("KeyN -> N, Digit1 -> 1, модификаторы без изменений", () => {
    expect(formatCombo("Ctrl+Shift+KeyN")).toBe("Ctrl+Shift+N");
    expect(formatCombo("Ctrl+Digit1")).toBe("Ctrl+1");
    // Backquote is not the character it types, so the hint would otherwise read
    // "Ctrl+Backquote" — the code's name rather than the key on the keyboard.
    expect(formatCombo("Ctrl+Backquote")).toBe("Ctrl+`");
  });
});

describe("findConflicts", () => {
  it("два действия с одинаковой (дефолтной) комбинацией — конфликт", () => {
    const binds = { view_notes: "Ctrl+KeyK" }; // rebound onto the palette's combination
    expect(findConflicts(binds, "view_notes", "Ctrl+KeyK")).toEqual(["palette"]);
  });

  it("уникальная комбинация — конфликтов нет", () => {
    expect(findConflicts({}, "palette", "Ctrl+KeyJ")).toEqual([]);
  });

  it("проверяемое действие само с собой не конфликтует", () => {
    expect(findConflicts({}, "palette", "Ctrl+KeyK")).toEqual([]);
  });
});
