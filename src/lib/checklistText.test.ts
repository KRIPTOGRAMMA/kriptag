import { describe, it, expect } from "vitest";
import {
  parseChecklist,
  formatChecklist,
  toggleLine,
  moveLineToEnd,
  lineIndexAt,
  removeLineAt,
  emptyAfterBackspace,
  dropEmptyLines,
  repairChecklistMarkup,
  CHECK_RE,
} from "./checklistText";

describe("parseChecklist", () => {
  it("читает отметки из префиксов", () => {
    const r = parseChecklist("[x] купить билеты\n[ ] собрать сумку");
    expect(r).toEqual([
      { title: "купить билеты", done: true },
      { title: "собрать сумку", done: false },
    ]);
  });

  it("строка без префикса — невыполненная подзадача", () => {
    // The main paste scenario: the list was copied from anywhere and the user is
    // under no obligation to mark it up by hand.
    expect(parseChecklist("собрать сумку")).toEqual([
      { title: "собрать сумку", done: false },
    ]);
  });

  it("пустые строки не становятся подзадачами", () => {
    // The user presses Enter before starting to type — normal, and not a reason to
    // create a nameless subtask.
    expect(parseChecklist("[x] раз\n\n\n[ ] два\n   \n")).toHaveLength(2);
  });

  it("понимает markdown-список и заглавную X", () => {
    const r = parseChecklist("- [X] раз\n  * [ ] два\n[]три");
    expect(r).toEqual([
      { title: "раз", done: true },
      { title: "два", done: false },
      { title: "три", done: false },
    ]);
  });

  it("текст в скобках внутри названия не путается с префиксом", () => {
    // The prefix only counts at the start of a line; brackets further along are data.
    const r = parseChecklist("[ ] позвонить [важно]");
    expect(r).toEqual([{ title: "позвонить [важно]", done: false }]);
  });
});

describe("formatChecklist", () => {
  it("пишет префикс и невыполненным тоже", () => {
    // Without `[ ]` there would be nothing to tick with: one would have to type the
    // brackets by hand instead of putting an x between ready-made ones.
    expect(formatChecklist([{ title: "раз", done: false }])).toBe("[ ] раз");
  });

  it("парсинг и сборка обратимы", () => {
    const text = "[x] раз\n[ ] два";
    expect(formatChecklist(parseChecklist(text))).toBe(text);
  });
});

describe("toggleLine", () => {
  it("переключает нужную строку, не трогая соседние", () => {
    expect(toggleLine("[ ] раз\n[ ] два", 1)).toBe("[ ] раз\n[x] два");
    expect(toggleLine("[x] раз\n[ ] два", 0)).toBe("[ ] раз\n[ ] два");
  });

  it("нумерация пропускает пустые строки, как и разбор", () => {
    // The index comes from the rendered list, which has no empty lines; if toggleLine
    // counted them the tick would land on the wrong line.
    expect(toggleLine("[ ] раз\n\n[ ] два", 1)).toBe("[ ] раз\n\n[x] два");
  });

  it("строке без префикса префикс дописывается", () => {
    expect(toggleLine("раз", 0)).toBe("[x] раз");
  });

  it("сохраняет пустые строки и текст без разметки", () => {
    // Reassembling from parseChecklist would lose the empty line, which is why
    // toggleLine edits the text in place rather than rebuilding it.
    expect(toggleLine("[ ] раз\n\n[ ] два", 0)).toBe("[x] раз\n\n[ ] два");
  });
});

// CHECK_RE's bounds are a contract with the editor: the range hidden behind the
// checkbox widget is computed from them. If a bound slips the user either sees the
// brackets or loses the subtask's first letter.
describe("CHECK_RE (границы разметки для виджета)", () => {
  it("совпадение покрывает скобки и пробел после них, но не текст", () => {
    const m = CHECK_RE.exec("[x] купить билеты");
    expect(m?.[0]).toBe("[x] ");
    expect(m?.[1]).toBe("");
    expect(m?.[2]).toBe("x");
  });

  it("ведущий маркер списка уходит в первую группу", () => {
    // Group 1 is not hidden, or a nested item's indent would disappear.
    const m = CHECK_RE.exec("  - [ ] собрать сумку");
    expect(m?.[1]).toBe("  - ");
    expect(m?.[0]).toBe("  - [ ] ");
  });

  it("строка без разметки не совпадает", () => {
    expect(CHECK_RE.test("просто текст")).toBe(false);
  });

  it("текст в скобках внутри строки не совпадает", () => {
    expect(CHECK_RE.test("позвонить [важно]")).toBe(false);
  });
});

describe("lineIndexAt", () => {
  it("определяет строку под кареткой", () => {
    const text = "[ ] раз\n[ ] два\n[ ] три";
    expect(lineIndexAt(text, 0)).toBe(0);
    expect(lineIndexAt(text, text.indexOf("два") + 1)).toBe(1);
    expect(lineIndexAt(text, text.length)).toBe(2);
  });

  it("каретка в начале новой пустой строки не относится к предыдущей", () => {
    const text = "[ ] раз\n";
    expect(lineIndexAt(text, text.length)).toBe(1);
  });
});

// The `[ ] ` markup is hidden behind a widget, so to the user "erase the subtask"
// means erasing the visible text. That used to leave a `[ ] ` line behind: an empty
// line with a checkbox on screen and nothing in the data.
describe("removeLineAt", () => {
  it("удаляет строку целиком вместе со скрытой разметкой", () => {
    const text = "[ ] раз\n[x] два\n[ ] три";
    expect(removeLineAt(text, text.indexOf("два"))).toBe("[ ] раз\n[ ] три");
  });

  it("на месте удалённой строки не остаётся пустой", () => {
    const text = "[ ] раз\n[ ] два";
    const out = removeLineAt(text, text.indexOf("два"));
    expect(out).toBe("[ ] раз");
    expect(out.split("\n")).toHaveLength(1);
  });

  // The first line is a special case: there is no newline before it, so the one
  // after it must go instead, or the second line is left with a blank above it.
  it("удаление первой строки поднимает вторую наверх", () => {
    const text = "[ ] раз\n[ ] два\n[ ] три";
    expect(removeLineAt(text, 0)).toBe("[ ] два\n[ ] три");
  });

  it("удаление единственной строки очищает поле", () => {
    expect(removeLineAt("[ ] одна", 3)).toBe("");
  });

  it("удаление последней строки не оставляет висящего перевода", () => {
    const text = "[ ] раз\n[ ] два";
    expect(removeLineAt(text, text.length)).toBe("[ ] раз");
  });

  // Deleting a line is an operation on text rather than on the list of subtasks:
  // empty lines the user typed but never filled in are preserved.
  it("не трогает соседние строки и их отметки", () => {
    const text = "[x] сделано\n[ ] лишняя\n[x] тоже сделано";
    expect(removeLineAt(text, text.indexOf("лишняя")))
      .toBe("[x] сделано\n[x] тоже сделано");
  });

  it("удалённая строка исчезает из разбора", () => {
    const text = "[ ] раз\n[ ] два";
    const after = removeLineAt(text, text.indexOf("два"));
    expect(parseChecklist(after).map(i => i.title)).toEqual(["раз"]);
  });
});


// The main deletion scenario: the user erases a subtask FROM THE END rather than
// putting the caret at the start of the line. When the last letter goes the subtask
// must go with it — otherwise an empty line with a checkbox stays on screen and one
// more press on the invisible brackets is required.
describe("emptyAfterBackspace", () => {
  it("удаление последней буквы опустошает строку", () => {
    // "[ ] я" with the caret at the end (col 5); we erase "я"
    expect(emptyAfterBackspace("[ ] я", 5)).toBe(true);
  });

  it("пока текст остаётся — это обычное укорачивание", () => {
    expect(emptyAfterBackspace("[ ] хлеб", 8)).toBe(false);
    expect(emptyAfterBackspace("[ ] хлеб", 6)).toBe(false);
  });

  it("разметка за текст не считается", () => {
    // The caret right after the hidden brackets: there is no text at all.
    expect(emptyAfterBackspace("[ ] ", 4)).toBe(true);
    expect(emptyAfterBackspace("[x] ", 4)).toBe(true);
  });

  it("каретка в начале строки с текстом — строка не пустеет", () => {
    // Backspace here would join the lines, but the subtask's text has not gone anywhere.
    expect(emptyAfterBackspace("[ ] хлеб", 0)).toBe(false);
  });

  it("пробелы текстом не считаются", () => {
    expect(emptyAfterBackspace("[ ]   ", 6)).toBe(true);
  });

  it("работает на строке без разметки", () => {
    expect(emptyAfterBackspace("я", 1)).toBe(true);
    expect(emptyAfterBackspace("да", 2)).toBe(false);
  });

  it("удаление в середине слова строку не опустошает", () => {
    expect(emptyAfterBackspace("[ ] хлеб", 6)).toBe(false);
  });

  // The indent and the markdown marker are markup too: the line `  - [ ] я` becomes
  // empty when its only letter is deleted rather than counting as non-empty because
  // of the hyphen.
  it("markdown-маркер и отступ за текст не считаются", () => {
    expect(emptyAfterBackspace("  - [ ] я", 9)).toBe(true);
    expect(emptyAfterBackspace("  - [ ] яд", 10)).toBe(false);
  });
});

// An empty subtask (`[ ] ` after Enter) and a bare blank line (Shift+Enter) are
// visible on screen, but parseChecklist drops them and they never reach the DB — a
// discrepancy between what is seen and what is saved.
describe("dropEmptyLines", () => {
  it("убирает пустую подзадачу с чекбоксом", () => {
    expect(dropEmptyLines("[ ] раз\n[ ] \n[ ] два")).toBe("[ ] раз\n[ ] два");
  });

  it("убирает голую пустую строку", () => {
    expect(dropEmptyLines("[ ] раз\n\n[ ] два")).toBe("[ ] раз\n[ ] два");
  });

  it("убирает строку из одних пробелов", () => {
    expect(dropEmptyLines("[ ] раз\n   \n[ ] два")).toBe("[ ] раз\n[ ] два");
  });

  it("сохраняет отметки и порядок", () => {
    expect(dropEmptyLines("[x] раз\n\n[ ] два\n[x] три"))
      .toBe("[x] раз\n[ ] два\n[x] три");
  });

  it("непустой список не меняется", () => {
    const text = "[x] раз\n[ ] два";
    expect(dropEmptyLines(text)).toBe(text);
  });

  it("список из одних пустых строк схлопывается в пустоту", () => {
    expect(dropEmptyLines("[ ] \n\n[ ] ")).toBe("");
    expect(dropEmptyLines("")).toBe("");
  });

  // A deliberately accepted side effect of reassembling through formatChecklist: the
  // markup is normalized to one form. The user does not see it (a widget hides it),
  // and inconsistency could only arrive by pasting from the clipboard.
  it("нормализует разметку вставленного markdown", () => {
    expect(dropEmptyLines("- [X] раз\n  * [ ] два")).toBe("[x] раз\n[ ] два");
  });

  // A line with no markup is a subtask (that is how pasting a list works), so the
  // cleanup does not drop it but adds the prefix.
  it("строка без разметки становится подзадачей, а не мусором", () => {
    expect(dropEmptyLines("раз\n\nдва")).toBe("[ ] раз\n[ ] два");
  });
});

// A custom Backspace covered one key while Ctrl+Backspace (delete word) went past
// it and ate the brackets from the inside, leaving a visible stump "[ " in the line.
// The markup must never be shown to the user, so the result of any deletion is
// repaired rather than a list of keys.
describe("repairChecklistMarkup", () => {
  it("целую разметку не трогает", () => {
    const text = "[ ] раз\n[x] два";
    expect(repairChecklistMarkup(text)).toBe(text);
  });

  it("восстанавливает префикс у строки с огрызком и текстом", () => {
    expect(repairChecklistMarkup("[ раз")).toBe("[ ] раз");
    expect(repairChecklistMarkup("] раз")).toBe("[ ] раз");
    expect(repairChecklistMarkup("[x раз")).toBe("[ ] раз");
  });

  // A stump must be separated from the text by a space. `[раз` is deliberately left
  // alone: telling a leftover of the markup from a word begun with a bracket is
  // impossible, and corrupting typed text is worse than leaving a bracket.
  it("скобка, приклеенная к слову, — текст, а не огрызок", () => {
    expect(repairChecklistMarkup("[раз")).toBe("[раз");
    expect(repairChecklistMarkup("[важно] сделать")).toBe("[важно] сделать");
    expect(repairChecklistMarkup("[xyz] код")).toBe("[xyz] код");
    expect(repairChecklistMarkup("[TODO] дело")).toBe("[TODO] дело");
  });

  it("строка из одного огрызка становится пустой", () => {
    // dropEmptyLines will remove it later, when focus is lost.
    expect(repairChecklistMarkup("[ ")).toBe("");
    expect(repairChecklistMarkup("[")).toBe("");
  });

  it("чинит только испорченные строки, соседние сохраняет", () => {
    expect(repairChecklistMarkup("[x] раз\n[ два\n[ ] три"))
      .toBe("[x] раз\n[ ] два\n[ ] три");
  });

  it("сохраняет отступ и markdown-маркер", () => {
    expect(repairChecklistMarkup("  - [ дело")).toBe("  - [ ] дело");
  });

  // This function's main risk is eating legitimate text. A line with no markup is a
  // valid subtask (that is how pasting a list works) and must not be touched.
  it("строку без разметки не трогает", () => {
    expect(repairChecklistMarkup("просто текст")).toBe("просто текст");
    expect(repairChecklistMarkup("раз\nдва")).toBe("раз\nдва");
  });

  it("скобки в середине текста — это данные, а не разметка", () => {
    expect(repairChecklistMarkup("[ ] позвонить [важно]"))
      .toBe("[ ] позвонить [важно]");
    expect(repairChecklistMarkup("позвонить [важно]"))
      .toBe("позвонить [важно]");
  });

  it("результат починки читается разбором как подзадача", () => {
    const fixed = repairChecklistMarkup("[ купить хлеб");
    expect(parseChecklist(fixed)).toEqual([{ title: "купить хлеб", done: false }]);
  });
});

// A ticked subtask sinks to the bottom, as in Xiaomi Notes. Called only from the
// checkbox click: doing it while typing would slide a line out from under the caret.
describe("moveLineToEnd", () => {
  it("отмеченная строка уходит в конец, остальные сохраняют порядок", () => {
    expect(moveLineToEnd("[x] раз\n[ ] два\n[ ] три", 0))
      .toBe("[ ] два\n[ ] три\n[x] раз");
  });

  it("из середины — тоже в конец", () => {
    expect(moveLineToEnd("[ ] раз\n[x] два\n[ ] три", 1))
      .toBe("[ ] раз\n[ ] три\n[x] два");
  });

  // Otherwise clicking the last checkbox would rewrite the text to no effect.
  it("последняя строка остаётся на месте", () => {
    const raw = "[ ] раз\n[x] два";
    expect(moveLineToEnd(raw, 1)).toBe(raw);
  });

  it("несуществующий индекс ничего не меняет", () => {
    const raw = "[ ] раз\n[ ] два";
    expect(moveLineToEnd(raw, 5)).toBe(raw);
    expect(moveLineToEnd(raw, -1)).toBe(raw);
  });

  // A trailing empty line is what the user is about to type into (Enter is already
  // pressed). The ticked line goes BEFORE it, or the caret would end up below a
  // completed subtask.
  it("пустой хвост остаётся последним", () => {
    expect(moveLineToEnd("[x] раз\n[ ] два\n", 0))
      .toBe("[ ] два\n[x] раз\n");
  });

  it("строки без разметки считаются наравне с размеченными", () => {
    expect(moveLineToEnd("раз\nдва\nтри", 0)).toBe("два\nтри\nраз");
  });

  // The move must not lose anything: parsing before and after yields the same set.
  it("состав подзадач сохраняется, меняется только порядок", () => {
    const raw = "[x] раз\n[ ] два\n[ ] три";
    const before = parseChecklist(raw);
    const after = parseChecklist(moveLineToEnd(raw, 0));
    expect(after).toHaveLength(before.length);
    expect([...after].sort((a, b) => a.title.localeCompare(b.title)))
      .toEqual([...before].sort((a, b) => a.title.localeCompare(b.title)));
  });
});
