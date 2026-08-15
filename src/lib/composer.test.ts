import { describe, it, expect } from "vitest";
import { parseComposer, parseTaskText, matchCategoryQuery, splitSubtaskLines } from "./composer";

// Quick capture splits off the "☐" lines too, but its title lives in a field of
// its own, so the first line of the description field is already description.
// parseComposer ate it silently there: the task was saved with description = null
// and the text simply vanished.
describe("splitSubtaskLines", () => {
  it("первая строка остаётся текстом, а не уходит в заголовок", () => {
    expect(splitSubtaskLines("выехать пораньше\n☐ забрать билеты")).toEqual({
      text: "выехать пораньше",
      subtasks: ["забрать билеты"],
    });
  });

  it("текст без ☐ возвращается целиком", () => {
    expect(splitSubtaskLines("первая\nвторая")).toEqual({
      text: "первая\nвторая",
      subtasks: [],
    });
  });

  it("пустые ☐-строки отбрасываются", () => {
    expect(splitSubtaskLines("☐ \n☐ реальная")).toEqual({
      text: "",
      subtasks: ["реальная"],
    });
  });
});

describe("parseComposer", () => {
  it("одна строка — только название", () => {
    expect(parseComposer("купить хлеб")).toEqual({
      title: "купить хлеб",
      description: "",
      subtasks: [],
    });
  });

  it("обычные строки после названия — описание, ☐-строки — подзадачи", () => {
    const src = "составить план\nна следующую неделю\n☐ пункт один\n☐ пункт два";
    expect(parseComposer(src)).toEqual({
      title: "составить план",
      description: "на следующую неделю",
      subtasks: ["пункт один", "пункт два"],
    });
  });

  it("подзадачи могут идти вперемешку с текстом, порядок сохраняется", () => {
    const src = "задача\n☐ раз\nописание\n☐ два";
    const d = parseComposer(src);
    expect(d.subtasks).toEqual(["раз", "два"]);
    expect(d.description).toBe("описание");
  });

  it("пустые ☐-строки и ведущие пустые строки отбрасываются", () => {
    const src = "\n\nзадача\n☐ \n☐ реальная";
    expect(parseComposer(src)).toEqual({
      title: "задача",
      description: "",
      subtasks: ["реальная"],
    });
  });

  it("☐ с отступом тоже подзадача", () => {
    expect(parseComposer("t\n  ☐ вложенная").subtasks).toEqual(["вложенная"]);
  });

  it("пустой и null-вход не падают", () => {
    expect(parseComposer("")).toEqual({ title: "", description: "", subtasks: [] });
    expect(parseComposer(null as unknown as string).title).toBe("");
  });
});

describe("parseTaskText", () => {
  const now = new Date("2026-07-23T10:00:00"); // a Thursday

  it("чистое название без маркеров — priority/category/tags/deadline пустые", () => {
    const d = parseTaskText("купить хлеб", now);
    expect(d).toEqual({ title: "купить хлеб", priority: null, categoryQuery: null, tags: [], deadline: null });
  });

  it("!приоритет вырезается из названия и распознаётся", () => {
    expect(parseTaskText("сделать отчёт !высокий", now).title).toBe("сделать отчёт");
    expect(parseTaskText("сделать отчёт !высокий", now).priority).toBe("High");
    expect(parseTaskText("!критический баг", now).priority).toBe("Critical");
    expect(parseTaskText("!низкий таск", now).priority).toBe("Low");
    expect(parseTaskText("!средний таск", now).priority).toBe("Medium");
  });

  it("@категория вырезается и возвращается как сырой запрос", () => {
    const d = parseTaskText("созвон @работа", now);
    expect(d.title).toBe("созвон");
    expect(d.categoryQuery).toBe("работа");
  });

  it("#тег вырезается и добавляется в tags, можно несколько", () => {
    const d = parseTaskText("задача #важное #срочно-по-факту", now);
    expect(d.title).toBe("задача");
    expect(d.tags).toEqual(["важное", "срочно-по-факту"]);
  });

  it("завтра + время — дедлайн на завтра в указанное время", () => {
    const d = parseTaskText("завтра 15:00 созвон", now);
    expect(d.title).toBe("созвон");
    expect(d.deadline).toEqual(new Date("2026-07-24T15:00:00"));
  });

  it("послезавтра без времени — дедлайн на начало дня", () => {
    const d = parseTaskText("послезавтра важный созвон", now);
    expect(d.deadline).toEqual(new Date("2026-07-25T00:00:00"));
  });

  it("сегодня + время", () => {
    const d = parseTaskText("сегодня 18:30 сдать отчёт", now);
    expect(d.deadline).toEqual(new Date("2026-07-23T18:30:00"));
  });

  it("только время без даты — дедлайн сегодня в это время", () => {
    const d = parseTaskText("созвон 14:00", now);
    expect(d.deadline).toEqual(new Date("2026-07-23T14:00:00"));
  });

  it("день недели — ближайшее будущее вхождение", () => {
    // now is Thursday 2026-07-23, so "пятница" (Friday) means tomorrow (2026-07-24)
    const d = parseTaskText("встреча пятница 10:00", now);
    expect(d.deadline).toEqual(new Date("2026-07-24T10:00:00"));
  });

  it("без даты/времени — deadline null", () => {
    expect(parseTaskText("просто задача", now).deadline).toBeNull();
  });

  it("все маркеры вместе, порядок токенов не важен, заголовок собирается из оставшихся слов", () => {
    const d = parseTaskText("завтра 15:00 созвон !высокий @работа #важное", now);
    expect(d.title).toBe("созвон");
    expect(d.priority).toBe("High");
    expect(d.categoryQuery).toBe("работа");
    expect(d.tags).toEqual(["важное"]);
    expect(d.deadline).toEqual(new Date("2026-07-24T15:00:00"));
  });

  it("невалидное время (25:99) не распознаётся как маркер, остаётся в названии", () => {
    const d = parseTaskText("тест 25:99 слово", now);
    expect(d.title).toBe("тест 25:99 слово");
    expect(d.deadline).toBeNull();
  });

  it("неизвестное слово после ! или @ не считается маркером-приоритетом, но @ и # всегда режутся", () => {
    // ! followed by an unrecognized word: the token stays in the title as is (we could not parse it)
    const d = parseTaskText("сделать !абракадабра", now);
    expect(d.priority).toBeNull();
    expect(d.title).toBe("сделать !абракадабра");
  });
});

describe("matchCategoryQuery", () => {
  const categories = [
    { id: "Work", name: "Работа" },
    { id: "abc-123", name: "Спорт" },
  ];

  it("сопоставляет по имени без учёта регистра", () => {
    expect(matchCategoryQuery(categories, "работа")).toBe("Work");
    expect(matchCategoryQuery(categories, "РАБОТА")).toBe("Work");
  });

  it("сопоставляет по id", () => {
    expect(matchCategoryQuery(categories, "abc-123")).toBe("abc-123");
  });

  it("подрезает обрамляющую пунктуацию", () => {
    expect(matchCategoryQuery(categories, "«спорт»")).toBe("abc-123");
  });

  it("нет совпадения — null", () => {
    expect(matchCategoryQuery(categories, "отдых")).toBeNull();
  });
});
