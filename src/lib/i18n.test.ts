import { describe, it, expect } from "vitest";
import { translate, detectLang, seededName, SEEDED_CATEGORY_IDS, LANGS } from "./i18n";
import { EN } from "./i18n.en";

describe("translate", () => {
  it("русский возвращает ключ как есть — он и есть оригинал", () => {
    expect(translate("Задачи", "ru")).toBe("Задачи");
    // even when the key is absent from the EN dictionary
    expect(translate("Никогда не переводившаяся строка", "ru"))
      .toBe("Никогда не переводившаяся строка");
  });

  it("английский берёт перевод из словаря", () => {
    expect(translate("Задачи", "en")).toBe("Tasks");
    expect(translate("Заметки", "en")).toBe("Notes");
  });

  // The key property of the "key = Russian text" scheme: an unfinished translation
  // degrades into a readable Russian string rather than into "tasks.empty_state" or
  // emptiness on screen.
  it("отсутствующий перевод отдаёт русский оригинал, а не пустоту", () => {
    const missing = "Строка, которой точно нет в словаре 12345";
    expect(translate(missing, "en")).toBe(missing);
    expect(translate(missing, "en")).not.toBe("");
  });

  it("подстановка переменных работает в обоих языках", () => {
    expect(translate("Очищено записей: {n}", "ru", { n: 3 })).toBe("Очищено записей: 3");
    expect(translate("Очищено записей: {n}", "en", { n: 3 })).toBe("Rows cleared: 3");
  });

  it("повторяющаяся переменная подставляется везде", () => {
    expect(translate("{a} и ещё {a}", "ru", { a: "раз" })).toBe("раз и ещё раз");
  });

  it("лишние переменные не ломают строку, отсутствующие остаются как есть", () => {
    expect(translate("Просто текст", "ru", { unused: 1 })).toBe("Просто текст");
    expect(translate("Значение: {x}", "ru")).toBe("Значение: {x}");
  });
});

// Categories and statuses come from the DB, so no static test over the sources ever
// saw them — "Работа" and "В работе" stayed Russian in an English interface. We
// translate only the seeded rows, and only while the user has not touched them.
describe("seededName", () => {
  it("посевная категория переводится по id", () => {
    expect(seededName("category", "Work", "Работа", "en")).toBe("Work");
    expect(seededName("category", "Health", "Здоровье", "en")).toBe("Health");
  });

  it("посевной статус переводится по id", () => {
    expect(seededName("status", "InProgress", "В работе", "en")).toBe("In progress");
    expect(seededName("status", "Archived", "Архив", "en")).toBe("Archive");
  });

  it("на русском отдаёт исходное имя", () => {
    expect(seededName("category", "Work", "Работа", "ru")).toBe("Работа");
    expect(seededName("status", "Done", "Готово", "ru")).toBe("Готово");
  });

  // The key property: a name the user wrote is their text. It must not be translated
  // no matter how it coincides with a seeded one.
  //
  // "Работа" here exercises the id check in its pure form: the id is not a seeded
  // one, so the comparison against the original does not come into play (a uuid is
  // absent from the originals table) while the name is translatable — meaning only
  // the id cutoff can be what fires.
  it("пользовательская категория не переводится", () => {
    expect(seededName("category", "b3f1c2a4-uuid", "Работа", "en")).toBe("Работа");
    expect(seededName("category", "b3f1c2a4-uuid", "Мои дела", "en")).toBe("Мои дела");
  });

  it("переименованная посевная категория не переводится", () => {
    // The id is a seeded one but the name no longer matches, which means the user
    // changed it. Translating here would hide their edit from them.
    //
    // The new names are taken FROM THE DICTIONARY deliberately: on a string absent
    // from it translate() would return the string as is anyway and the test would
    // pass even without the comparison against the original — it would be testing the
    // dictionary's incompleteness rather than the guard.
    expect(seededName("category", "Work", "Здоровье", "en")).toBe("Здоровье");
    expect(seededName("status", "Done", "Архив", "en")).toBe("Архив");
  });

  // kind is part of the key rather than decoration: without it the status "Done"
  // would be found among the categories and vice versa. The pairs below exist in
  // neither table.
  it("kind участвует в поиске оригинала", () => {
    expect(seededName("category", "Done", "Готово", "en")).toBe("Готово");
    expect(seededName("status", "Work", "Работа", "en")).toBe("Работа");
  });

  // Settings disables the rename field based on this list. Should it drift from the
  // originals table, a seeded category would become editable and the very first edit
  // would write the English translation into the DB over the Russian original.
  it("список id для Настроек совпадает с посевными категориями", () => {
    expect([...SEEDED_CATEGORY_IDS].sort())
      .toEqual(["Health", "Home", "Other", "Study", "Work"]);
  });

  it("все посевные имена есть в словаре EN", () => {
    for (const name of ["Работа", "Учёба", "Дом", "Здоровье", "Другое", "В работе", "Готово", "Архив"]) {
      expect(EN[name], `нет перевода для посевного имени «${name}»`).toBeTypeOf("string");
    }
  });
});

describe("detectLang", () => {
  it("русская локаль — русский", () => {
    expect(detectLang("ru")).toBe("ru");
    expect(detectLang("ru-RU")).toBe("ru");
    expect(detectLang("RU-ru")).toBe("ru");
  });

  // Anything not Russian counts as English: to a non-Russian user English is more
  // useful than Russian, and the converse does not hold.
  it("любая другая локаль — английский", () => {
    expect(detectLang("en-US")).toBe("en");
    expect(detectLang("de")).toBe("en");
    expect(detectLang("")).toBe("en");
  });
});

describe("словарь EN", () => {
  it("в нём нет пустых переводов", () => {
    for (const [key, val] of Object.entries(EN)) {
      expect(val.trim(), `пустой перевод для «${key}»`).not.toBe("");
    }
  });

  // A translation identical to the original usually means a forgotten string rather
  // than a deliberate decision. The exceptions are words shared by both languages.
  it("переводы не совпадают с русским оригиналом", () => {
    const sameAsKey = Object.entries(EN).filter(([k, v]) => k === v);
    expect(sameAsKey).toEqual([]);
  });

  it("плейсхолдеры перевода совпадают с оригиналом", () => {
    const re = /\{(\w+)\}/g;
    for (const [key, val] of Object.entries(EN)) {
      const inKey = [...key.matchAll(re)].map(m => m[1]).sort();
      const inVal = [...val.matchAll(re)].map(m => m[1]).sort();
      expect(inVal, `плейсхолдеры разошлись в «${key}»`).toEqual(inKey);
    }
  });

  it("оба языка объявлены в LANGS", () => {
    expect(LANGS.map(l => l.id).sort()).toEqual(["en", "ru"]);
  });
});

// The main risk of filling the dictionary gradually is wrapping a string in t() and
// forgetting to add the translation. The English interface then silently shows a
// Russian string: the mechanism is designed that way (degradation beats emptiness),
// but for files ALREADY marked up that is a bug rather than an unfinished
// translation.
//
// So what is checked is not "the whole UI is translated" but something narrower and
// more honest: in the files we declared translated, every key is in the dictionary.
// The list of files grows as localization proceeds — a new file is added right here.
describe("покрытие словаря по размеченным файлам", () => {
  // The files are read through import.meta.glob (Vite, `as: "raw"`) rather than
  // node:fs: there is no @types/node in this project and pulling it in for a single
  // test would be disproportionate — glob is typed by Vite itself and works in the
  // jsdom environment.
  const SOURCES = import.meta.glob("/src/**/*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

  // The list grows as localization proceeds: a file we have declared fully
  // translated is added here.
  const LOCALIZED = [
    "/src/App.svelte",
    "/src/views/Settings.svelte",
    "/src/views/Tasks.svelte",
    "/src/views/Notes.svelte",
    "/src/views/Calendar.svelte",
    "/src/views/Dashboard.svelte",
    "/src/lib/components/TaskModal.svelte",
    "/src/lib/components/QuickCapture.svelte",
    "/src/views/Today.svelte",
    "/src/views/Onboarding.svelte",
    "/src/lib/components/LiveMarkdownEditor.svelte",
    "/src/lib/components/RoutinesModal.svelte",
    "/src/lib/components/PomodoroWidget.svelte",
    "/src/lib/components/SearchOverlay.svelte",
    "/src/lib/components/NotificationPanel.svelte",
    "/src/lib/components/TrackingWidget.svelte",
    "/src/lib/components/ModelDownloader.svelte",
    "/src/lib/components/VoiceButton.svelte",
    "/src/views/NotesGraph.svelte",
    "/src/lib/components/TaskHistoryDetail.svelte",
    "/src/lib/components/WindowControls.svelte",
    "/src/lib/components/ChecklistEditor.svelte",
  ];

  it("все объявленные файлы найдены — путь не устарел", () => {
    for (const f of LOCALIZED) {
      expect(SOURCES[f], `не найден файл ${f}`).toBeTypeOf("string");
    }
  });

  // The test above catches only strings ALREADY wrapped in t(). A Russian string
  // someone forgot to wrap was invisible to it — that is exactly how "localization is
  // finished" diverged from reality: the user found Russian text in Settings, the
  // sidebar, the graph and the tooltips. This test looks from the other side: in
  // marked-up files no Cyrillic may remain outside t()/tr().
  //
  // What deliberately does NOT count as a violation:
  // - comments: this test is about interface strings, not about the language the
  //   code is explained in. Comments have their own guard — comments.test.ts;
  // - <style> (Cyrillic appears there only in comments);
  // - the keys inside t("...") themselves — those are the dictionary keys;
  // - blocks marked `/* i18n-ok */`, which are translated where they are rendered
  //   rather than where they are declared (the NAV and palette command lists:
  //   `{t(item.label)}`). The marker lifts the check from the block up to the nearest
  //   `];` line and must be explicit: otherwise the test either stays silent about
  //   real omissions or demands "fixing" working code.
  //   `i18n-ok-line` is the same permission for exactly one following line, for a
  //   string that is not in a list at all: the language names on the first step of
  //   the onboarding are written in their own language on purpose, so "Русский"
  //   must never be translated. The array form cannot express that — it would skip
  //   to the next `];`, which in markup may be the end of the file.
  it("в размеченных файлах нет кириллицы вне t()", () => {
    const offenders: string[] = [];
    for (const file of LOCALIZED) {
      let src = SOURCES[file] ?? "";
      src = src.replace(/<style[\s\S]*?<\/style>/g, "");
      src = src.replace(/<!--\s*i18n-ok-line[^>]*-->/g, "@@I18N_OK_LINE@@");
      src = src.replace(/<!--[\s\S]*?-->/g, "");
      src = src.replace(/@@I18N_OK_LINE@@/g, "i18n-ok-line");
      // The order matters: the `/* i18n-ok */` marker is protected BEFORE block
      // comments are stripped, otherwise it is removed along with them and the block
      // counts as a violation again.
      src = src.replace(/\/\*\s*i18n-ok\s*\*\//g, "@@I18N_OK@@");
      src = src.replace(/\/\*[\s\S]*?\*\//g, "");
      src = src.replace(/(^|[^:"'`\\])\/\/.*$/gm, (m, p1) =>
        m.includes("@@I18N_OK@@") ? `${p1}@@I18N_OK@@` : p1);
      src = src.replace(/@@I18N_OK@@/g, "i18n-ok");
      // The contents of t("...") / tr("...") are stripped — they are required to be
      // Russian. Single quotes are needed as much as double ones: inside a markup
      // attribute (`title="{t('Поиск')} (Ctrl+K)"`) there is no other way to write it,
      // and without this branch the test would count an already-translated string as a
      // violation.
      src = src.replace(/(?<![\w.])tr?\((["'])(?:(?!\1).)*\1/g, "t()");
      let skipUntilClose = false;
      let skipNextLine = false;
      for (const [i, line] of src.split("\n").entries()) {
        if (skipNextLine) { skipNextLine = false; continue; }
        if (line.includes("i18n-ok-line")) { skipNextLine = true; continue; }
        if (line.includes("i18n-ok")) { skipUntilClose = true; continue; }
        if (skipUntilClose) {
          if (/^\s*\];/.test(line)) skipUntilClose = false;
          continue;
        }
        if (!/[а-яА-Я]/.test(line)) continue;
        offenders.push(`${file}:${i + 1}: ${line.trim()}`);
      }
    }
    expect(offenders, `не обёрнуто в t():\n${offenders.join("\n")}`).toEqual([]);
  });

  // The help (help.ts) is pure data with no t(): it is translated at render time in
  // Settings.svelte (`{t(item.desc)}`). The previous two tests therefore do not see it
  // at all, and without this check a new help topic would silently stay Russian in an
  // English interface — which is exactly what happened.
  it("вся справка (help.ts) есть в словаре EN", () => {
    const HELP_SRC = import.meta.glob("/src/lib/help.ts", {
      query: "?raw", import: "default", eager: true,
    }) as Record<string, string>;
    const src = HELP_SRC["/src/lib/help.ts"] ?? "";
    expect(src, "help.ts не найден — путь устарел").not.toBe("");
    const missing: string[] = [];
    for (const m of src.matchAll(/(?:title|term|desc):\s*\n?\s*"((?:[^"\\]|\\.)*)"/g)) {
      const key = m[1].replace(/\\"/g, '"');
      if (!(key in EN)) missing.push(key);
    }
    expect(missing, `нет перевода:\n${missing.join("\n")}`).toEqual([]);
  });

  // keybinds.ts follows the same scheme as help.ts: pure data translated at render
  // time (`{t(action.label)}` in Settings). The action names are visible on the
  // "Hotkeys" tab, and without this check a new action would stay Russian.
  it("названия действий (keybinds.ts) есть в словаре EN", () => {
    const KB = import.meta.glob("/src/lib/keybinds.ts", {
      query: "?raw", import: "default", eager: true,
    }) as Record<string, string>;
    const src = KB["/src/lib/keybinds.ts"] ?? "";
    expect(src, "keybinds.ts не найден — путь устарел").not.toBe("");
    const missing: string[] = [];
    for (const m of src.matchAll(/label: "([^"]+)"/g)) {
      if (!(m[1] in EN)) missing.push(m[1]);
    }
    expect(missing, `нет перевода:\n${missing.join("\n")}`).toEqual([]);
  });

  // The model descriptions come from Rust (commands/model.rs) and are translated at
  // render time in ModelDownloader.svelte. None of the tests above sees them: the
  // .svelte file itself is marked up and clean while the Cyrillic lives outside /src —
  // the very same blind spot that left the AI tab Russian after "localization is
  // finished".
  it("описания моделей (model.rs) есть в словаре EN", () => {
    const RS = import.meta.glob("/src-tauri/src/commands/model.rs", {
      query: "?raw", import: "default", eager: true,
    }) as Record<string, string>;
    const src = RS["/src-tauri/src/commands/model.rs"] ?? "";
    expect(src, "model.rs не найден — путь устарел").not.toBe("");
    const missing: string[] = [];
    let found = 0;
    for (const m of src.matchAll(/description:\s*"((?:[^"\\]|\\.)*)"/g)) {
      found++;
      const key = m[1].replace(/\\"/g, '"');
      if (!(key in EN)) missing.push(key);
    }
    // Otherwise the test "passes" by ceasing to find anything: if the string format in
    // model.rs changes, an empty list would silently pass for an absence of omissions.
    expect(found, "в model.rs не найдено ни одного описания — изменился формат").toBeGreaterThan(0);
    expect(missing, `нет перевода:\n${missing.join("\n")}`).toEqual([]);
  });

  it("каждый t(\"...\") из размеченных файлов есть в словаре EN", () => {
    const missing: string[] = [];
    for (const file of LOCALIZED) {
      const src = SOURCES[file] ?? "";
      // `tr(` is for Calendar.svelte, where `t` is taken by the task variable in
      // {#each} and the translation helper is imported as `tr`. Without this branch the
      // test would silently count the file as translated without checking a single one
      // of its keys.
      // (?<![\w.]) keeps split("...") and import("...") from being caught.
      for (const m of src.matchAll(/(?<![\w.])tr?\("([^"]+)"/g)) {
        if (!(m[1] in EN)) missing.push(`${file}: ${m[1]}`);
      }
    }
    expect(missing).toEqual([]);
  });
});
