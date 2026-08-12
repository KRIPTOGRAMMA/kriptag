import { test, expect, type Page, type Locator } from "@playwright/test";
import { KEYBIND_ACTIONS, formatCombo } from "../src/lib/keybinds";

// Смоук-набор против vite dev с моком Tauri (__TAURI_INTERNALS__).
// Rust-слой в этих тестах не участвует — он покрыт `cargo test`.

async function withMock(page: Page) {
  await page.addInitScript({ path: "./e2e/tauri-mock.js" });
}

// Страж немокнутых команд (v0.9.80). Мок копит имена неизвестных ему команд в
// window.__unknownInvokes (tauri-mock.js:111) и возвращает undefined — экран
// рисует пустоту, а тест остаётся зелёным. Массив существовал с самого начала,
// но его никто не читал, поэтому детектор молчал.
//
// Здесь проверяется РАНТАЙМ-случай: команда вызвана этим тестом и не мокнута.
// Более сильный случай — команда есть в Rust и забыта в моке, даже если её никто
// не вызывает — ловится cargo-тестом src-tauri/src/mock_guard.rs.
//
// page.evaluate обёрнут в try/catch: тест мог закрыть страницу сам (или упасть
// до её открытия), и падение стража поверх настоящей ошибки только запутает.
test.afterEach(async ({ page }) => {
  let unknown: string[] = [];
  try {
    unknown = await page.evaluate(() => (window as any).__unknownInvokes ?? []);
  } catch {
    return; // страница уже закрыта — проверять нечего
  }
  expect(
    [...new Set(unknown)],
    "команды, вызванные тестом, но отсутствующие в e2e/tauri-mock.js: " +
      "мок вернул undefined, и UI отрисовал пустоту вместо данных",
  ).toEqual([]);
});

// Ждёт, пока симуляция графа остынет. Координаты узлов пишутся напрямую через
// setAttribute мимо реактивности Svelte, поэтому «остыл» видно только по тому,
// что transform перестал меняться от кадра к кадру. Явного признака в DOM нет
// (rafId — переменная модуля), и заводить его ради тестов не нужно.
// Заменяет waitForTimeout(1500), который был просто НЕДОСТАТОЧЕН: замер показал,
// что граф из 2 узлов остывает за ~2.5с, из 3 — за ~6.3с. Тесты драга проходили
// на движущемся графе — их спасало только то, что сдвиг от драга (>80px)
// заведомо больше остаточного дрейфа.
async function waitForGraphSettled(page: Page, timeout = 12000) {
  const snapshot = () => page.evaluate(() =>
    [...document.querySelectorAll(".node")].map(n => n.getAttribute("transform") ?? "").join("|")
  );
  const deadline = Date.now() + timeout;
  let prev = await snapshot();
  let stableFrames = 0;
  while (Date.now() < deadline) {
    await page.waitForTimeout(50);
    const next = await snapshot();
    // Двух совпавших подряд замеров мало: симуляция может «проползать» медленно.
    stableFrames = next === prev ? stableFrames + 1 : 0;
    prev = next;
    if (stableFrames >= 2) return;
  }
  throw new Error("граф не остыл за отведённое время");
}

// Сид состояния мока: кладётся в localStorage ДО tauri-mock.js,
// который подхватывает существующий __mock_db.
async function seedDb(page: Page, db: object) {
  await page.addInitScript((json) => {
    localStorage.setItem("__mock_db", json);
  }, JSON.stringify(db));
}

async function createTask(page: Page, title: string) {
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  await page.getByPlaceholder("Название задачи").fill(title);
  await page.getByRole("button", { name: "Создать" }).click();
}

// Списки в модалке задачи и в мете заметки — свой компонент Select, а не родной
// <select>: WebKitGTK рисует попап родного системной темой GTK, и на тёмной теме
// он оставался белым. Поэтому здесь клик по кнопке и клик по пункту, а не
// selectOption. Родные <select> (фильтры, Настройки, быстрый ввод) на месте, и
// тесты на них по-прежнему ходят через selectOption.
// Сам список рендерится position:fixed на верхнем уровне, вне модалки, поэтому
// пункт ищется по странице целиком, даже когда кнопка искалась внутри scope.
async function pickOption(page: Page, scope: Page | Locator, label: string, option: string | RegExp) {
  // exact: «Проект» иначе цепляет и кнопку раздела «Проекты», а «Категория» —
  // заголовки на других экранах.
  await scope.getByRole("button", { name: label, exact: true }).click();
  await page.getByRole("option", { name: option, exact: true }).first().click();
}

// Списки модалки задачи рисуются своим компонентом именно потому, что родной
// <select> в WebKitGTK красится системной темой GTK. Тест не может это увидеть
// (Playwright гоняет headless-Chromium, где родной попап как раз слушается CSS),
// поэтому проверяем то, что проверяемо: список — наша разметка на токенах темы,
// он вне модалки и потому не обрезается ею, и выбор реально меняет значение.
test("модалка задачи: списки свои, а не родные, и выбор доходит до поля", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await createTask(page, "задача со списками");
  await taskByTitle(page, "задача со списками").locator(".task-main").dblclick();

  const modal = page.locator(".modal");
  // Ни одного родного <select>: если хоть один останется, рядом со своими он и
  // будет тем самым белым списком, из-за которого всё затевалось.
  await expect(modal.locator("select")).toHaveCount(0);

  await modal.getByRole("button", { name: "Приоритет" }).click();
  const list = page.locator('[role="listbox"]');
  await expect(list).toBeVisible();

  // Модалка прокручивается и обрезает содержимое, поэтому список должен быть
  // position:fixed — тогда overflow предка ему не указ. Проверяем не место в
  // DOM (там он как раз внутри .modal), а наблюдаемое следствие: список виден
  // целиком и по нему реально попадает клик.
  const vis = await list.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.x + r.width / 2, r.y + 5);
    return {
      position: getComputedStyle(el).position,
      inWindow: r.top >= 0 && r.bottom <= window.innerHeight && r.left >= 0,
      clickable: el.contains(hit) || el === hit,
    };
  });
  expect(vis.position).toBe("fixed");
  expect(vis.inWindow, "список вылез за окно").toBe(true);
  expect(vis.clickable, "список перекрыт или обрезан").toBe(true);

  // Фон берётся из токенов темы, а не остаётся системным.
  const bg = await list.evaluate(el => getComputedStyle(el).backgroundColor);
  expect(bg).not.toBe("rgba(0, 0, 0, 0)");

  await page.getByRole("option", { name: "Критический" }).click();
  await expect(list).toHaveCount(0);
  await expect(modal.getByRole("button", { name: "Приоритет" })).toContainText("Критический");

  // Escape закрывает список, не трогая модалку.
  await modal.getByRole("button", { name: "Приоритет" }).click();
  await expect(page.locator('[role="listbox"]')).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator('[role="listbox"]')).toHaveCount(0);
  await expect(modal).toBeVisible();

  // И выбор долетает до задачи, а не остаётся в UI.
  // exact: иначе локатор ловит ещё и «Сохранить как шаблон».
  await page.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(page.locator(".modal")).toHaveCount(0);
  await taskByTitle(page, "задача со списками").locator(".task-main").dblclick();
  await expect(page.locator(".modal").getByRole("button", { name: "Приоритет" }))
    .toContainText("Критический");
});

// Длинное название задачи растягивало список блокеров до своей ширины (замерено
// 759px против 458 сейчас), и он уезжал за правый край окна — а в вебвью туда
// нечем прокрутить. Чинится с двух сторон: потолок ширины + обрезка текста в
// пункте, и зажим по ширине СПИСКА, а не кнопки, которая его открыла.
test("списки: длинное название задачи не выносит список за край окна", async ({ page }) => {
  const long = "Очень длинное название задачи которое совершенно точно не помещается ".repeat(6);
  const now = new Date().toISOString();
  await seedDb(page, {
    notes: [],
    settings: { onboarding_complete: true },
    tasks: [
      { id: "long-one", title: long, status: "Todo", priority: "Medium", category: "Work",
        tags: [], hidden: false, subtasks: [], created_at: now, updated_at: now },
      { id: "target", title: "цель", status: "Todo", priority: "Medium", category: "Work",
        tags: [], hidden: false, subtasks: [], created_at: now, updated_at: now },
    ],
  });
  await withMock(page);
  await page.goto("/");

  for (const w of [1280, 800]) {
    await page.setViewportSize({ width: w, height: 800 });
    await page.locator(".task-row", { hasText: "цель" }).locator(".task-main").dblclick();
    await page.locator(".modal")
      .getByRole("button", { name: "Добавить блокер...", exact: true }).click();

    const geom = await page.locator('[role="listbox"]').evaluate((el) => {
      const r = el.getBoundingClientRect();
      const opt = el.querySelector(".opt") as HTMLElement;
      const or = opt.getBoundingClientRect();
      const btn = document.querySelector(".sel-btn[aria-expanded=true]") as HTMLElement;
      return {
        right: r.right, left: r.left, winW: window.innerWidth, listW: r.width,
        btnW: btn.getBoundingClientRect().width,
        // Пункт обрезается, а не тянет список.
        optClamped: opt.scrollWidth > opt.clientWidth + 1,
        optH: or.height,
        // Ширину пункту задаёт контейнер, а не собственный текст. Проверять надо
        // именно это: -webkit-line-clamp в WebKitGTK не сработал, и пункт тянул
        // список во весь экран — в Chromium этого было не видно.
        optW: opt.getBoundingClientRect().width,
        listScrollW: el.scrollWidth,
        listClientW: el.clientWidth,
      };
    });

    expect(geom.right, `список за правым краем на ${w}px`).toBeLessThanOrEqual(geom.winW);
    expect(geom.left, `список за левым краем на ${w}px`).toBeGreaterThanOrEqual(0);
    expect(geom.optClamped, "длинный пункт не обрезан").toBe(true);
    // Ключевое: пункт не шире списка и ничего не торчит вбок. Прежняя проверка
    // ловила только высоту в две строки — а она держалась на line-clamp, которого
    // в WebKitGTK нет, так что тест был зелёным при сломанном экране.
    expect(geom.optW, "пункт шире списка").toBeLessThanOrEqual(geom.listW);
    expect(geom.listScrollW, "содержимое торчит вбок")
      .toBeLessThanOrEqual(geom.listClientW + 1);
    expect(geom.optH, "пункт выше одной строки").toBeLessThan(40);
    // Потолок ширины — отдельная защита от зажима по краю: без него список
    // остаётся в окне, но растягивается на всю его ширину (замерено 1276px из
    // 1280). Список задач шириной в экран — это не список, а полоса.
    // Не уже кнопки: список внутри собственного контрола выглядит сломанным.
    expect(geom.listW, `список раздулся на ${w}px`)
      .toBeLessThanOrEqual(Math.max(geom.btnW, 420) + 1);

    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
  }
});

// Пункты обрезаны до одной строки, поэтому список задач — стопка почти одинакового
// текста, и границу между соседями глазу не за что зацепить. Разделяет линия.
test("списки: пункты разделены линией, подсветка её не разрывает", async ({ page }) => {
  const now = new Date().toISOString();
  const titles = ["первая задача", "вторая задача", "третья задача"];
  await seedDb(page, {
    notes: [],
    settings: { onboarding_complete: true },
    tasks: [
      ...titles.map((t, i) => ({ id: `b${i}`, title: t, status: "Todo", priority: "Medium",
        category: "Work", tags: [], hidden: false, subtasks: [], created_at: now, updated_at: now })),
      { id: "tgt", title: "цель", status: "Todo", priority: "Medium", category: "Work",
        tags: [], hidden: false, subtasks: [], created_at: now, updated_at: now },
    ],
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".task-row")
    .filter({ has: page.getByText("цель", { exact: true }) }).locator(".task-main").dblclick();
  await page.locator(".modal")
    .getByRole("button", { name: "Добавить блокер...", exact: true }).click();

  const sep = await page.evaluate(() => {
    const opts = [...document.querySelectorAll(".opt")] as HTMLElement[];
    const cs = (i: number) => getComputedStyle(opts[i]);
    return {
      // У первого линии нет: иначе список открывался бы черта под собственным
      // отступом. У остальных — есть, и по токену темы, а не «серым вообще».
      first: cs(0).borderTopWidth,
      second: cs(1).borderTopWidth,
      color: cs(1).borderTopColor,
      // Радиус 0, иначе заливка отходит от линии и та выглядит рваной по краям.
      radius: cs(1).borderTopLeftRadius,
    };
  });
  expect(sep.first, "линия под первым пунктом").toBe("0px");
  expect(sep.second, "пункты не разделены").toBe("1px");
  expect(sep.color).not.toBe("rgba(0, 0, 0, 0)");
  expect(sep.radius).toBe("0px");

  // Подсветанная строка съедает касающиеся её линии — свою и следующего пункта, —
  // иначе полоса выглядит перечёркнутой по обоим концам. Соседи сверху не трогаем.
  await page.locator(".opt").nth(1).hover();
  // Ждём не появления класса, а конца перехода: у button в app.css есть
  // transition border-color 0.12s, и замер сразу после hover ловил цвет на
  // полпути (замерено rgba(...,0.835)) — тест падал на верной вёрстке.
  await expect(page.locator(".opt.active")).toHaveCount(1);
  await expect
    .poll(async () => page.locator(".opt.active")
      .evaluate(el => getComputedStyle(el).borderTopColor))
    .toBe("rgba(0, 0, 0, 0)");
  const hov = await page.evaluate(() => {
    const opts = [...document.querySelectorAll(".opt")] as HTMLElement[];
    const i = opts.findIndex(o => o.classList.contains("active"));
    const cs = (k: number) => getComputedStyle(opts[k]);
    return {
      i,
      own: cs(i).borderTopColor,
      next: cs(i + 1).borderTopColor,
      prev: cs(i - 1).borderTopColor,
      // Высоты не должны прыгать: линия становится прозрачной, а не исчезает.
      heights: opts.map(o => Math.round(o.getBoundingClientRect().height)),
    };
  });
  expect(hov.own, "своя линия не спрятана").toBe("rgba(0, 0, 0, 0)");
  expect(hov.next, "линия следующего не спрятана").toBe("rgba(0, 0, 0, 0)");
  expect(hov.prev, "съедена лишняя линия сверху").not.toBe("rgba(0, 0, 0, 0)");
  expect(new Set(hov.heights.slice(1)).size, "строки прыгают по высоте").toBe(1);
});

// Диалоги «Проекты» и «Новый умный список» несли class="modal dialog", но .dialog
// в Tasks.svelte никто не объявлял — только .modal в app.css. Получалась карточка
// без внутренних отступов: заголовок в пикселе от рамки, кнопки впритык к краю.
// Выглядело как обрезанное окно. Оба должны совпадать с модалкой задачи рядом.
test("диалоги Задач: отступы как у модалки задачи, а не впритык к рамке", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    notes: [],
    settings: { onboarding_complete: true },
    tasks: [{ id: "t1", title: "задача", status: "Todo", priority: "Medium", category: "Work",
      tags: [], hidden: false, subtasks: [], created_at: now, updated_at: now }],
  });
  await withMock(page);
  await page.goto("/");

  // Замер берём с самого диалога: padding и то, насколько отступает от рамки его
  // первый элемент. Второе важнее — именно оно и выглядело обрезанным.
  const probe = () => page.locator(".modal.dialog").evaluate((d) => {
    const r = d.getBoundingClientRect();
    const first = d.firstElementChild as HTMLElement;
    return {
      padding: getComputedStyle(d).padding,
      maxWidth: getComputedStyle(d).maxWidth,
      firstGap: Math.round(first.getBoundingClientRect().left - r.left),
    };
  });

  await page.locator(".task-row").first().locator(".task-main").dblclick();
  const modal = await probe();
  await page.keyboard.press("Escape");

  await page.getByRole("button", { name: "Проекты", exact: true }).click();
  const projects = await probe();
  await page.locator(".modal.dialog").getByRole("button", { name: "Закрыть" }).click();

  await page.locator(".smart-list-add").click();
  const smart = await probe();

  // Эталон — модалка задачи: у неё отступы были всегда.
  expect(modal.padding, "у модалки задачи пропали отступы").not.toBe("0px");
  expect(modal.firstGap).toBeGreaterThan(10);

  for (const [name, d] of [["Проекты", projects], ["Умный список", smart]] as const) {
    expect(d.padding, `${name}: контент впритык к рамке`).toBe(modal.padding);
    expect(d.maxWidth, `${name}: ширина не как у модалки задачи`).toBe(modal.maxWidth);
    expect(d.firstGap, `${name}: заголовок прижат к рамке`).toBe(modal.firstGap);
  }
});

// Булевы настройки — свой тумблер (Switch.svelte): родной чекбокс в WebKitGTK
// рисуется системной темой GTK мимо токенов. Настоящий <input> под ним остался
// ради семантики, но он прозрачный, поэтому check()/uncheck() до него не
// достучатся — кликать надо по обёртке. Состояние по-прежнему читается с input.
async function flipSwitch(scope: Page | Locator, labelText: string) {
  await scope.locator("label", { hasText: labelText }).locator(".sw").first().click();
}

// Булевы настройки рисуются своим тумблером, а не родным чекбоксом: в WebKitGTK
// родной берёт форму и цвет от системной темы GTK, мимо токенов приложения (та же
// причина, что у выпадающих списков и галочек подзадач). Увидеть это тестом
// нельзя — Playwright гоняет Chromium, — поэтому проверяем проверяемое: разметка
// наша, состояние доступно клавиатуре и скринридеру, клик по обёртке переключает.
test("настройки: булевы значения — свой тумблер, доступный с клавиатуры", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Уведомления" }).click();

  const label = page.locator("label", { hasText: "Фокус-режим: авто-пауза уведомлений" });
  const sw = label.locator(".sw").first();
  const input = label.locator("input[type=checkbox]").first();

  await expect(sw).toBeVisible();
  // Родной <input> остался ради семантики, но спрятан прозрачностью, а не
  // display:none — иначе он выпал бы из дерева доступности вместе с состоянием.
  await expect(input).toHaveCSS("opacity", "0");
  await expect(input).toHaveCount(1);

  const before = await input.isChecked();
  await sw.click();
  await expect(input).toBeChecked({ checked: !before });

  // Клавиатура работает через тот же настоящий чекбокс.
  await input.focus();
  await page.keyboard.press("Space");
  await expect(input).toBeChecked({ checked: before });

  // Заливка и положение — из токенов темы, а не системные.
  await sw.click();
  const look = await sw.evaluate((el) => {
    const track = el.querySelector(".track") as HTMLElement;
    const knob = el.querySelector(".knob") as HTMLElement;
    return {
      trackBg: getComputedStyle(track).backgroundColor,
      knobShift: getComputedStyle(knob).transform,
    };
  });
  expect(look.trackBg).not.toBe("rgba(0, 0, 0, 0)");
  // Ручка уехала вправо: состояние читается положением, а не только цветом.
  expect(look.knobShift, "ручка не сдвинулась").not.toBe("none");

  // Ручка светлая и с тенью в ОБОИХ состояниях: цвет у неё не меняется, глаз
  // следит за движением, а тень отделяет её от залитой акцентом дорожки.
  const knob = sw.locator(".knob");
  const knobOn = await knob.evaluate(el => ({
    bg: getComputedStyle(el).backgroundColor,
    shadow: getComputedStyle(el).boxShadow,
  }));
  expect(knobOn.shadow, "у ручки нет тени").not.toBe("none");
  await sw.click();
  const knobOff = await knob.evaluate(el => getComputedStyle(el).backgroundColor);
  expect(knobOff, "ручка меняет цвет вместо того, чтобы ездить").toBe(knobOn.bg);

  // Дорожка ВЫКЛЮЧЕННОГО тумблера обязана иметь видимую рамку: на светлой теме
  // карточка белая, и без рамки выключенное состояние сливается с фоном
  // (посчитанный контраст заливки к карточке — 1.11, то есть его нет).
  //
  // Читается объявленное правило, а не вычисленный стиль элемента: под курсором
  // .sw:hover красит рамку акцентом, и вычисленное значение показывало рамку
  // видимой даже при border-color: transparent в самом правиле — проверено
  // поломкой, тест проходил на сломанном коде.
  const declared = await page.evaluate(() => {
    for (const sheet of [...document.styleSheets]) {
      let rules: CSSRuleList;
      try { rules = sheet.cssRules; } catch { continue; }
      for (const r of [...rules] as CSSStyleRule[]) {
        if (!r.selectorText || !/\.track/.test(r.selectorText)) continue;
        if (/:hover|:checked|:focus/.test(r.selectorText)) continue;
        if (r.style.border || r.style.borderColor) {
          return r.style.border || r.style.borderColor;
        }
      }
    }
    return null;
  });
  expect(declared, "у выключенной дорожки не объявлена рамка").toBeTruthy();
  expect(declared, "рамка выключенной дорожки прозрачна")
    .not.toMatch(/transparent|rgba\(0, 0, 0, 0\)/);
});

// Язык и ИИ-провайдер — свой Select, а не родной <select>: в WebKitGTK попап
// родного рисуется системной темой GTK мимо токенов. Кнопка + пункт вместо
// selectOption. Список рендерится position:fixed на верхнем уровне, поэтому
// пункт ищется по странице, а не внутри метки.
async function pickSetting(page: Page, labelText: string, option: string | RegExp) {
  await page.locator("label", { hasText: labelText })
    .locator("button[aria-haspopup=listbox]").first().click();
  await page.getByRole("option", { name: option, exact: true }).first().click();
}

// В Настройках не должно остаться родных <select>: в WebKitGTK их попап рисует
// системная тема GTK мимо токенов приложения. Проверяется весь экран разом, а не
// отдельные списки, — иначе следующий добавленный список тихо проедет мимо.
test("настройки: родных списков не осталось, у темы своя подпись", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Все вкладки: секции скрыты классом, но в DOM присутствуют.
  const natives = await page.evaluate(() =>
    [...document.querySelectorAll(".settings select")].map(
      s => s.closest("label")?.textContent?.trim().slice(0, 30) ?? "?"));
  expect(natives, "остался родной <select>").toEqual([]);

  // Пресеты акцента — свой листбокс, а не <select>, и это не должно откатиться.
  await expect(page.locator(".preset-select")).toHaveCount(1);

  // У переключателя темы есть подпись, как у всего остального в этой панели.
  // Одних отступов не хватило: «Язык» сверху и «Пресеты акцента» снизу подписаны,
  // и безымянный ряд кнопок между ними читался как продолжение поля языка.
  const themeLabel = page.locator(".sub-label", { hasText: "Тема" }).first();
  await expect(themeLabel).toBeVisible();

  // Подпись оформлена и отбита так же, как соседняя «Пресеты акцента», иначе
  // рядом стояли бы два заголовка одного уровня, выглядящие по-разному.
  const rhythm = await page.evaluate(() => {
    const lang = [...document.querySelectorAll("label.field")]
      .find(l => l.textContent?.includes("Язык")) as HTMLElement;
    const labels = [...document.querySelectorAll(".sub-label")] as HTMLElement[];
    const theme = labels.find(l => l.textContent?.trim() === "Тема")!;
    const preset = labels.find(l => l.textContent?.includes("Пресеты"))!;
    const seg = document.querySelector(".theme-seg") as HTMLElement;
    const box = document.querySelector(".preset-select") as HTMLElement;
    const r = (e: HTMLElement) => e.getBoundingClientRect();
    const st = (e: HTMLElement) => {
      const c = getComputedStyle(e);
      return `${c.fontSize}/${c.color}`;
    };
    return {
      langToLabel: Math.round(r(theme).top - r(lang).bottom),
      themeLabelToControl: Math.round(r(seg).top - r(theme).bottom),
      presetLabelToControl: Math.round(r(box).top - r(preset).bottom),
      sameStyle: st(theme) === st(preset),
    };
  });
  expect(rhythm.sameStyle, "подпись темы оформлена иначе, чем соседняя").toBe(true);
  expect(rhythm.langToLabel, "подпись темы слиплась с полем языка").toBeGreaterThanOrEqual(8);
  // Подпись должна принадлежать своему контролу, а не висеть между двумя.
  expect(rhythm.themeLabelToControl).toBe(rhythm.presetLabelToControl);
});

// У главной кнопки по краю был виден шов. Причина не в градиенте, а в рамке:
// базовое правило задаёт `border: 1px solid`, .btn-primary гасила только цвет, а
// прозрачная рамка всё равно занимает свой пиксель — и при background-clip:
// border-box заливка рисуется ПОД ней, размазывая крайние цвета градиента по
// кромке. Лечится снятием рамки, а не подбором цвета.
test("кнопки: у главной нет прозрачной рамки и она не ниже соседей", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  const geom = await page.evaluate(() => {
    const prim = document.querySelector(".btn-primary") as HTMLElement;
    const cs = getComputedStyle(prim);
    // Соседи по той же строке: главная кнопка не должна выбиваться по высоте.
    const sibs = [...prim.parentElement!.querySelectorAll("button")] as HTMLElement[];
    const plain = sibs.find(b => !b.classList.contains("btn-primary")
      && !b.closest(".seg") && b !== prim);
    return {
      borderWidth: cs.borderTopWidth,
      borderColor: cs.borderTopColor,
      primH: Math.round(prim.getBoundingClientRect().height),
      plainH: plain ? Math.round(plain.getBoundingClientRect().height) : null,
    };
  });

  // Прозрачная рамка — именно то сочетание, которое давало шов.
  expect(
    geom.borderWidth !== "0px" && geom.borderColor === "rgba(0, 0, 0, 0)",
    "у .btn-primary снова прозрачная рамка — по краю будет шов",
  ).toBe(false);

  // Снятие рамки укорачивает кнопку на 2px, если не вернуть их паддингом.
  expect(geom.plainH, "не с чем сравнить высоту").not.toBeNull();
  expect(geom.primH, "главная кнопка не совпадает по высоте с соседней")
    .toBe(geom.plainH);
});

// Кнопки окна висят над правым верхним углом (WindowControls — position:fixed,
// прозрачный, 32px). Раньше каждый экран уворачивался от них сам: отступ справа в
// каждой шапке плюс вырез clip-path в карточке Заметок — единственном экране, чей
// корень это залитая панель. Вырез оставлял в скруглённом углу заметную ступеньку
// и читался как обрезка. Теперь полоса целиком принадлежит титлбару: контент
// начинается под ней, и уворачиваться никому не нужно.
test("кнопки окна: контент начинается под ними, а не обтекает их", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [],
    settings: { onboarding_complete: true },
    notes: [{ id: "n1", title: "заметка", content: "текст", tags: [],
              created_at: now, updated_at: now }],
  });
  await withMock(page);
  await page.setViewportSize({ width: 1400, height: 860 });
  await page.goto("/");
  await page.getByRole("button", { name: "Заметки" }).first().click();
  await page.locator(".note-item").first().click();
  await page.locator(".editor-head").waitFor();

  const notes = await page.evaluate(() => {
    const ctl = document.querySelector(".controls")!.getBoundingClientRect();
    const card = document.querySelector(".notes") as HTMLElement;
    // elementsFromPoint, а не elementFromPoint: сверху лежит прозрачный титлбар
    // со своей кнопкой, и одиночный вариант возвращал бы её, никогда не добираясь
    // до карточки под ней — проверка проходила бы и на сломанном коде.
    const under = (document.elementsFromPoint(
      ctl.left + ctl.width / 2, ctl.top + ctl.height / 2) as HTMLElement[])
      .some(e => e === card || card.contains(e));
    return {
      under,
      cardTop: Math.round(card.getBoundingClientRect().top),
      btnBottom: Math.round(ctl.bottom),
      // Угол остаётся скруглённым и невырезанным — именно это выглядело обрезкой.
      radius: parseFloat(getComputedStyle(card).borderTopRightRadius),
      clip: getComputedStyle(card).clipPath,
    };
  });

  expect(notes.under, "карточка заметок под кнопками окна").toBe(false);
  // Вплотную к нижнему краю кнопок, а не под всей полосой титлбара: кнопки 24px
  // и центрированы в 32px, так что ниже них остаются 4px пустоты — на залитой
  // панели они читались бы как зазор над ней, а не как воздух.
  expect(notes.cardTop, "карточка залезает на кнопки")
    .toBeGreaterThanOrEqual(notes.btnBottom);
  expect(notes.cardTop - notes.btnBottom, "карточка не дотянута до кнопок")
    .toBeLessThanOrEqual(6);
  expect(notes.radius, "у карточки пропало скругление").toBeGreaterThan(0);
  expect(notes.clip, "вернулся вырез угла").toBe("none");

  // Шапки экранов больше не резервируют полосу справа: она принадлежит титлбару,
  // и лишний отступ снова оставил бы пустоту, на которую жаловались раньше.
  await page.getByRole("button", { name: "Задачи" }).first().click();
  const tasks = await page.evaluate(() => {
    const head = document.querySelector(".page-head") as HTMLElement;
    const ctl = document.querySelector(".controls")!.getBoundingClientRect();
    const hr = head.getBoundingClientRect();
    return {
      padRight: parseFloat(getComputedStyle(head).paddingRight),
      headBelowButtons: Math.round(hr.top) >= Math.round(ctl.bottom),
    };
  });
  expect(tasks.padRight, "шапка Задач всё ещё резервирует полосу под кнопки")
    .toBeLessThan(40);
  expect(tasks.headBelowButtons, "шапка Задач заходит под кнопки").toBe(true);
});

// Сегменты .seg скруглены по краям: overflow:hidden у контейнера НЕ обрезает
// собственный фон дочерней кнопки по радиусу, и залитый акцентом крайний сегмент
// торчал прямыми углами из скруглённой рамки.
test("переключатели: крайние сегменты подхватывают скругление рамки", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  const radii = await page.evaluate(() => {
    const seg = document.querySelector(".seg") as HTMLElement;
    const btns = [...seg.querySelectorAll("button")] as HTMLElement[];
    const cs = (e: HTMLElement) => getComputedStyle(e);
    return {
      container: cs(seg).borderTopLeftRadius,
      first: cs(btns[0]).borderTopLeftRadius,
      last: cs(btns[btns.length - 1]).borderTopRightRadius,
      middleLeft: btns.length > 2 ? cs(btns[1]).borderTopLeftRadius : "0px",
    };
  });

  expect(parseFloat(radii.first), "первый сегмент не скруглён").toBeGreaterThan(0);
  expect(parseFloat(radii.last), "последний сегмент не скруглён").toBeGreaterThan(0);
  // Внутренние стыки остаются прямыми, иначе сегменты распадаются на пилюли.
  expect(radii.middleLeft).toBe("0px");
  // Скругление сегмента чуть меньше контейнерного — на толщину его рамки.
  expect(parseFloat(radii.first)).toBeLessThan(parseFloat(radii.container));
});

// Действия заметки уехали в меню «…» в шапке редактора: шесть иконок подряд
// стояли ровно под кнопками окна того же размера, и два набора читались как один.
// Zen остался снаружи — им пользуются во время чтения, и он должен работать
// изнутри zen-режима, где остальная шапка скрыта.
async function noteAction(page: Page, label: string | RegExp) {
  await page.getByRole("button", { name: "Действия с заметкой" }).click();
  await page.locator('[role="menuitem"]', { hasText: label }).first().click();
}

// Карточку задачи открывает ДВОЙНОЙ клик: одинарный теперь правит название прямо
// в строке. Хелпер, а не dblclick() по месту, чтобы следующая смена жеста была
// одной правкой, а не двадцатью.
async function openTaskCard(page: Page, title: string | RegExp) {
  await page.locator(".task-row", { hasText: title }).first().locator(".task-main").dblclick();
}

// Одинарный клик правит название прямо в строке, двойной открывает карточку.
// Название — то, что правят чаще всего, и ходить ради опечатки в модалку было
// самым долгим путём.
test("строка задачи: клик правит название, двойной клик открывает карточку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await createTask(page, "исходное название");

  const row = page.locator(".task-row", { hasText: "исходное название" });
  await row.locator(".task-main").click();

  // Модалка не открылась — правка идёт в строке, поле уже под кареткой.
  await expect(page.locator(".modal")).toHaveCount(0);
  const editor = page.locator(".task-title-edit");
  await expect(editor).toBeFocused();
  await expect(editor).toHaveValue("исходное название");

  await editor.fill("правленое название");
  await editor.press("Enter");
  await expect(page.locator(".task-title-edit")).toHaveCount(0);
  await expect(page.getByText("правленое название")).toBeVisible();

  // Escape откатывает, а не сохраняет.
  await page.locator(".task-row", { hasText: "правленое название" }).locator(".task-main").click();
  await page.locator(".task-title-edit").fill("это не должно сохраниться");
  await page.locator(".task-title-edit").press("Escape");
  await expect(page.locator(".task-title-edit")).toHaveCount(0);
  await expect(page.getByText("правленое название")).toBeVisible();
  await expect(page.getByText("это не должно сохраниться")).toHaveCount(0);

  // Уход фокуса сохраняет — как в быстром слоте.
  await page.locator(".task-row", { hasText: "правленое название" }).locator(".task-main").click();
  await page.locator(".task-title-edit").fill("сохранено по блюру");
  await page.locator(".page-title").click();
  await expect(page.locator(".task-title-edit")).toHaveCount(0);
  await expect(page.getByText("сохранено по блюру")).toBeVisible();

  // Пустое название не способ удалить задачу — откатывается к прежнему.
  await page.locator(".task-row", { hasText: "сохранено по блюру" }).locator(".task-main").click();
  await page.locator(".task-title-edit").fill("   ");
  await page.locator(".task-title-edit").press("Enter");
  await expect(page.getByText("сохранено по блюру")).toBeVisible();

  // Двойной клик открывает карточку и не оставляет за собой поле правки.
  await page.locator(".task-row", { hasText: "сохранено по блюру" }).locator(".task-main").dblclick();
  await expect(page.locator(".modal")).toBeVisible();
  await expect(page.locator(".task-title-edit")).toHaveCount(0);
});

// Действия строки задачи живут в контекстном меню по правой кнопке (v0.9.98):
// раньше это были шесть иконок, появлявшихся по наведению. Левый клик по строке
// по-прежнему открывает модалку, поэтому меню открывается только правой.
async function rowMenu(page: Page, taskTitle: string, item: string | RegExp) {
  await page.locator(".task-row", { hasText: taskTitle }).first().click({ button: "right" });
  await page.locator('[role="menuitem"]', { hasText: item }).first().click();
}

// Пресеты тем — свой listbox, а не <select>: цветной свотч в <option> не живёт.
// Выбор всегда через открытие списка, поэтому один помощник на все тесты.
// Настройки сохраняются сами (дебаунс 800 мс), кнопки «Сохранить» там больше нет.
// Тест обязан дождаться записи, иначе reload прочитает старое состояние.
async function waitSettingsSaved(page: Page) {
  await expect(page.locator(".autosave-note")).toContainText("Сохранено", { timeout: 5000 });
}

// Свои наборы — второй такой же список на экране, поэтому берётся по позиции.
function customPresetSelect(page: Page) {
  return page.locator(".preset-select").nth(1);
}

async function chooseCustomPreset(page: Page, name: string) {
  const sel = customPresetSelect(page);
  await sel.locator(".preset-trigger").click();
  await sel.locator(".preset-option", { hasText: name }).click();
}

async function choosePreset(page: Page, name: string) {
  await page.locator(".preset-trigger").click();
  await page.locator(".preset-option", { hasText: name }).click();
}

// Живой markdown-редактор (CodeMirror 6, v0.6.9) — contenteditable, не textarea.
// Заменяет весь текст: клик → выделить всё → напечатать.
function noteEditor(page: Page) {
  return page.locator(".cm-content");
}
async function fillNoteEditor(page: Page, text: string) {
  const editor = noteEditor(page);
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  // insertText (не keyboard.type) — вставляет многострочный текст одним куском,
  // без реальных Enter-нажатий. Печать \n через type() триггерит markdown-
  // расширение CodeMirror «продолжить маркер списка на новой строке», что
  // дублирует "- [ ] " в многострочных чек-листах при построчном наборе.
  await page.keyboard.insertText(text);
  // Строка с курсором рендерится сырым markdown (иначе редактировать вслепую) —
  // уводим курсор на новую пустую строку, чтобы виджеты (ссылки/жирный/итд)
  // на введённом тексте стали видимыми для проверок.
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n");
}

test("онбординг проходится до конца и больше не показывается", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: false } });
  await withMock(page);
  await page.goto("/");

  // v0.10.14: первый шаг — выбор языка. Он идёт до всего остального: дальше один
  // текст, и выбирать язык позже значило бы прочитать весь онбординг не на том.
  await expect(page.getByText("Выберите язык интерфейса")).toBeVisible();
  await expect(page.getByText("Русский")).toBeVisible();
  await expect(page.getByText("English")).toBeVisible();
  // Выбираем ДРУГОЙ язык, а не язык по умолчанию: с "ru" проверка сохранения
  // прошла бы и без записи — в сиде уже лежит "ru".
  await page.getByText("English").click();
  // Смена применяется сразу: остальной онбординг — сплошной текст, и увидеть
  // результат до конца настройки можно только так.
  await expect(page.getByText("Choose the interface language")).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();

  await expect(page.getByText("Welcome to Kriptag")).toBeVisible();
  await page.getByRole("button", { name: "Start setup" }).click();
  await page.getByRole("button", { name: "Next" }).click();
  // шаг 3 (Wayland) пропущен: is_wayland → false
  await page.getByRole("button", { name: "Next" }).click();
  // v0.9.64: шаг голосового ввода — необязательный, проходится насквозь
  await page.getByRole("button", { name: "Next" }).click();
  await page.getByRole("button", { name: "Start", exact: true }).click();

  // главный экран, флаг сохранён в «БД»
  await expect(page.getByRole("heading", { name: "Tasks" })).toBeVisible();
  const db = JSON.parse(await page.evaluate(() => localStorage.getItem("__mock_db")!));
  expect(db.settings.onboarding_complete).toBe(true);
  // Выбранный язык сохраняется, а не остаётся только в памяти.
  expect(db.settings.language, "язык из онбординга не сохранился").toBe("en");
});

// v0.10.14: страница с биндами композитора — только на Wayland. На X11 и Windows
// глобальные хоткеи регистрирует само приложение, и конфиг композитора там ничего
// не значит; шага быть не должно вовсе.
test("онбординг: страница биндов композитора только на Wayland", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: false } });
  await withMock(page);
  // Мок отдаёт is_wayland: false — подменяем только для этого теста, чтобы не
  // трогать общий помощник ради одного шага.
  await page.addInitScript(() => {
    const w = window as unknown as { __forceWayland?: boolean };
    w.__forceWayland = true;
  });
  await page.goto("/");

  // Проходим до шага с конфигом.
  await page.getByRole("button", { name: "Далее" }).click();
  await page.getByRole("button", { name: "Начать настройку" }).click();
  await page.getByRole("button", { name: "Далее" }).click();
  await expect(page.getByText("Мониторинг на Wayland")).toBeVisible();
  await page.getByRole("button", { name: "Далее" }).click();
  await expect(page.getByText("Автозагрузка и хоткеи")).toBeVisible();
  await page.getByRole("button", { name: "Далее" }).click();

  await expect(page.getByText("Хоткеи в композиторе")).toBeVisible();
  // Готовые строки, а не описание: их копируют целиком.
  const conf = page.locator(".conf-block pre").first();
  await expect(conf).toContainText("bind = CTRL SHIFT, N, exec, kriptag --quick-task");
  // Все четыре быстрых ввода, а не только задача.
  for (const flag of ["--quick-task", "--quick-note", "--quick-clip", "--quick-pinned"]) {
    await expect(conf, `в конфиге нет ${flag}`).toContainText(flag);
  }
  await expect(page.locator(".conf-block pre").nth(1), "нет варианта для Sway").toContainText("bindsym Control+Shift+n exec kriptag");
});

test("задача: создание, редактирование, выполнение, удаление из истории", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "тестовая задача");
  await expect(page.getByText("тестовая задача")).toBeVisible();

  // карточка открывается двойным кликом: одинарный правит название в строке
  await page.locator(".task-main", { hasText: "тестовая задача" }).dblclick();
  await expect(page.getByText("Редактировать задачу")).toBeVisible();
  await page.getByPlaceholder("Название задачи").fill("переименованная задача");
  // exact — иначе матчится и «Сохранить как шаблон» (шаблоны чеклистов, v0.8.3)
  await page.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(page.getByText("переименованная задача")).toBeVisible();
  await expect(page.getByText("тестовая задача")).toHaveCount(0);

  // подзадача добавляется у задачи без подзадач (чип «+» виден всегда — v0.6.1)
  await page.locator(".chip-sub").click();
  await page.locator(".task-sub-panel .checklist-editor").click();
  await page.keyboard.insertText("первый шаг");
  // v0.9.45: запись отложена на паузу набора
  await expect(page.locator(".chip-sub")).toHaveText(/0\/1/);

  await expect(page.locator(".chip-sub")).toHaveClass(/has-subs/);

  // Вторая подзадача ДО отметки первой: иначе первая была бы последней
  // неотмеченной и сама закрыла бы задачу (complete_if_all_subtasks_done), а
  // дальше этот тест выполняет её вручную и идёт за ней в историю и Корзину.
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.keyboard.insertText("второй шаг");
  await expect(page.locator(".chip-sub")).toHaveText(/0\/2/);

  // v0.8.2: чип с подзадачами визуально выделен; все выполнены — зеленеет.
  await page.locator(".task-sub-panel .cm-sub-checkbox").first().click();
  await expect(page.locator(".chip-sub")).toHaveText(/1\/2/);
  await expect(page.locator(".chip-sub")).not.toHaveClass(/subs-done/);

  // выполнение — уходит из активных, появляется в истории
  await page.locator(".task-check").click();
  await expect(page.locator(".task-main", { hasText: "переименованная" })).toHaveCount(0);
  await page.getByRole("button", { name: "История" }).click();
  await expect(page.getByText("переименованная задача")).toBeVisible();

  // удаление из истории — мягкое (v0.8.12): уходит из истории, но не исчезает насовсем
  await page.getByTitle("Удалить").click();
  await expect(page.getByText("переименованная задача")).toHaveCount(0);

  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(page.locator(".task-list.trash", { hasText: "переименованная задача" })).toBeVisible();
});

// v0.9.24: завершение родителя каскадом закрывает его чеклист — раньше
// v0.9.98: действия строки уехали в контекстное меню. Тест держит три вещи,
// каждая из которых ломалась бы молча: меню вообще открывается правой кнопкой,
// левый клик по-прежнему ведёт в модалку (жест старше меню), и Escape закрывает
// меню, не оставляя невидимый backdrop поверх списка.
test("контекстное меню задачи: правая кнопка открывает, Escape закрывает, левый клик по-прежнему модалка", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "задача с меню");

  const row = page.locator(".task-row", { hasText: "задача с меню" }).first();
  await row.click({ button: "right" });
  await expect(page.locator('[role="menu"]')).toBeVisible();
  await expect(page.locator('[role="menuitem"]', { hasText: "Удалить" })).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(page.locator('[role="menu"]')).toHaveCount(0);

  // Backdrop убран вместе с меню: иначе он бы перехватил этот клик.
  await page.locator(".task-main", { hasText: "задача с меню" }).dblclick();
  await expect(page.getByText("Редактировать задачу")).toBeVisible();
});

// v0.9.99: фильтр по категориям. Держит две вещи: что он вообще фильтрует и что
// выбор переживает перезагрузку (он лежит в localStorage рядом с фильтром
// проектов). Заодно — что повторный клик снимает фильтр, иначе из него нет
// выхода, кроме как через «Все».
test("фильтр по категориям: сужает список, снимается повторным кликом, переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "рабочая задача");
  await page.locator(".task-main", { hasText: "рабочая задача" }).dblclick();
  await pickOption(page, page, "Категория", "Работа");
  await page.getByRole("button", { name: "Сохранить", exact: true }).click();

  await createTask(page, "домашняя задача");
  await page.locator(".task-main", { hasText: "домашняя задача" }).dblclick();
  await pickOption(page, page, "Категория", "Дом");
  await page.getByRole("button", { name: "Сохранить", exact: true }).click();

  const catFilter = page.locator(".cat-filter");
  await catFilter.getByRole("button", { name: "Работа" }).click();
  await expect(page.locator(".task-main", { hasText: "рабочая задача" })).toBeVisible();
  await expect(page.locator(".task-main", { hasText: "домашняя задача" })).toHaveCount(0);

  // выбор пережил перезагрузку
  await page.reload();
  await expect(page.locator(".task-main", { hasText: "домашняя задача" })).toHaveCount(0);

  // повторный клик по той же категории снимает фильтр
  await catFilter.getByRole("button", { name: "Работа" }).click();
  await expect(page.locator(".task-main", { hasText: "домашняя задача" })).toBeVisible();
});

// v0.9.98: пункт «Удалить» в меню делает то же, что делала иконка ✕.
test("контекстное меню задачи: удаление уносит задачу из списка", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "задача на удаление");
  await rowMenu(page, "задача на удаление", "Удалить");

  await expect(page.locator(".task-main", { hasText: "задача на удаление" })).toHaveCount(0);
});

// в истории лежала Done-задача с невыполненными подзадачами.
test("завершение задачи проставляет done всем её подзадачам", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "уборка");
  await expect(page.getByText("уборка")).toBeVisible();

  await page.locator(".chip-sub").click();
  await page.locator(".task-sub-panel .checklist-editor").click();
  await page.keyboard.insertText("пропылесосить");
  await page.keyboard.press("Enter");
  await page.keyboard.insertText("вынести мусор");
  // ни одна не отмечена вручную
  await expect(page.locator(".chip-sub")).toHaveText(/0\/2/);

  await page.locator(".task-check").click();
  await expect(page.locator(".task-main", { hasText: "уборка" })).toHaveCount(0);

  // в истории чеклист закрыт целиком, хотя вручную не отмечали ничего
  await page.getByRole("button", { name: "История" }).click();
  await page.locator(".history .task-main", { hasText: "уборка" }).click();
  const modal = page.locator(".modal");
  await expect(modal.locator(".check-row")).toHaveCount(2);
  await expect(modal.locator(".check-row").nth(0).locator("input")).toBeChecked();
  await expect(modal.locator(".check-row").nth(1).locator("input")).toBeChecked();
});

// v0.9.26: заметка из буфера обмена (Ctrl+Shift+B). Отдельная точка входа
// quick-task.html — то же окно быстрого ввода, что и для Ctrl+Shift+N/M.
test("заметка из буфера: окно открывается предзаполненным и сохраняет заметку", async ({ page }) => {
  // Сид ДО tauri-mock.js: мок читает localStorage один раз при загрузке,
  // поэтому init-скрипт с состоянием обязан быть зарегистрирован раньше.
  await seedDb(page, { tasks: [], notes: [], projects: [], quickMode: "clipboard" });
  await page.addInitScript(() => {
    (window as any).__mockClipboard = "Идея для доклада\nразобрать примеры\nи выводы";
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  // первая строка — заголовок, остальное — тело
  await expect(page.locator("input")).toHaveValue("Идея для доклада");
  await expect(page.locator("textarea")).toHaveValue("разобрать примеры\nи выводы");
  await expect(page.locator(".clip-hint")).toBeVisible();

  // подсказка снимается, как только текст правят руками
  await page.locator("input").fill("Идея для доклада (правка)");
  await expect(page.locator(".clip-hint")).toHaveCount(0);

  await page.getByRole("button", { name: "Создать" }).click();
  const notes = await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).notes.map((n: any) => n.title));
  expect(notes).toContain("Идея для доклада (правка)");
});

// Скопированная ссылка — самый частый случай этого хоткея, и голый URL в
// заголовке нечитаем в списке заметок. Такой буфер целиком уходит в тело.
test("заметка из буфера: скопированная ссылка попадает в тело, заголовок пуст", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], projects: [], quickMode: "clipboard" });
  await page.addInitScript(() => {
    (window as any).__mockClipboard = "https://example.com/article?id=42";
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  await expect(page.locator("input")).toHaveValue("");
  await expect(page.locator("textarea")).toHaveValue("https://example.com/article?id=42");
  // фокус в пустом заголовке — можно сразу печатать название
  await expect(page.locator("input")).toBeFocused();

  // сохранение без заголовка не блокируется, название падает в фолбэк
  await page.getByRole("button", { name: "Создать" }).click();
  const notes = await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).notes);
  expect(notes[0].title).toBe("Без названия");
  expect(notes[0].content).toBe("https://example.com/article?id=42");
});

test("заметка из буфера: пустой буфер даёт обычную пустую заметку, а не ошибку", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], projects: [], quickMode: "clipboard" });
  await page.addInitScript(() => {
    (window as any).__mockClipboard = "   \n\n  ";
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  await expect(page.locator("input")).toHaveValue("");
  await expect(page.locator("textarea")).toHaveValue("");
  await expect(page.locator(".clip-hint")).toHaveCount(0);
  await expect(page.locator(".error")).toHaveCount(0);
});

// v0.9.25: taskStore.error выставлялся, но нигде не рендерился — упавшая
// операция выглядела как «кнопка не работает». Теперь ошибка видна и, что
// не менее важно, снимается после первой же успешной операции.
test("ошибки задач видны в UI и пропадают после успешной операции", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "первая");
  await expect(page.locator(".task-error")).toHaveCount(0);

  // следующее завершение падает, как упала бы Rust-команда
  await page.evaluate(() => {
    (window as any).__mockFailNext = { cmd: "complete_task", msg: "Задача не найдена: xyz" };
  });
  await page.locator(".task-check").first().click();

  await expect(page.locator(".task-error")).toContainText("Задача не найдена: xyz");
  // задача осталась на месте — но теперь понятно, почему
  await expect(page.locator(".task-main", { hasText: "первая" })).toBeVisible();

  // успешное завершение снимает баннер (раньше error только выставлялся
  // и висел бы навсегда)
  await page.locator(".task-check").first().click();
  await expect(page.locator(".task-error")).toHaveCount(0);
  await expect(page.locator(".task-main", { hasText: "первая" })).toHaveCount(0);
});

test("ошибку задач можно закрыть крестиком", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "вторая");
  await page.evaluate(() => {
    (window as any).__mockFailNext = { cmd: "complete_task", msg: "Что-то пошло не так" };
  });
  await page.locator(".task-check").first().click();
  await expect(page.locator(".task-error")).toBeVisible();

  await page.locator(".task-error button").click();
  await expect(page.locator(".task-error")).toHaveCount(0);
});

// v0.9.70: AppError отдаёт «<русский префикс>: <детали>», и до этой версии
// префикс уезжал в интерфейс мимо словаря — англоязычный пользователь видел
// русский текст. Переводится только префикс: детали приходят из sqlx/io и
// остаются как есть.
test("технический префикс ошибки переводится, детали остаются", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");
  await page.getByRole("button", { name: /Tasks/ }).first().click();

  await page.evaluate(() => {
    (window as any).__mockFailNext = {
      cmd: "create_task",
      msg: "Ошибка базы данных: no such table: tasks",
    };
  });
  await page.getByRole("button", { name: /New|Новая/ }).first().click();
  await page.getByPlaceholder(/Task title|Название задачи/).fill("упадёт");
  await page.getByRole("button", { name: /^(Create|Создать)$/ }).click();

  const banner = page.locator(".task-error");
  await expect(banner).toContainText("Database error");
  // детали от sqlx не переводятся и не теряются
  await expect(banner).toContainText("no such table: tasks");
  await expect(banner).not.toContainText("Ошибка базы данных");
});

// Доменное сообщение — не технический префикс, даже если в нём есть двоеточие.
// Наивный перевод «головы до двоеточия» изуродовал бы его.
test("доменная ошибка не расчленяется по двоеточию", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");
  await page.getByRole("button", { name: /Tasks/ }).first().click();

  await page.evaluate(() => {
    (window as any).__mockFailNext = { cmd: "create_task", msg: "Задача не найдена: abc" };
  });
  await page.getByRole("button", { name: /New|Новая/ }).first().click();
  await page.getByPlaceholder(/Task title|Название задачи/).fill("вторая");
  await page.getByRole("button", { name: /^(Create|Создать)$/ }).click();

  await expect(page.locator(".task-error")).toContainText("Задача не найдена: abc");
});

// v0.9.69: часовая шкала собирается через pad2 из общего datetime.ts. Юнит-тесты
// проверяют саму функцию, но не то, что она доехала до разметки — а сломанный
// паддинг здесь выглядит как «0:00, 1:00» и портит вёрстку колонки.
test("часовая шкала календаря пронумерована с ведущим нулём", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();

  await expect(page.locator(".hour-mark", { hasText: "00:00" })).toBeVisible();
  await expect(page.locator(".hour-mark", { hasText: "09:00" })).toBeVisible();
  await expect(page.locator(".hour-mark", { hasText: "23:00" })).toBeVisible();
  // без паддинга получилось бы "9:00"
  await expect(page.locator(".hour-mark", { hasText: /^9:00$/ })).toHaveCount(0);
});

// v0.9.68: тот же дефект, что чинили в v0.9.25 для задач, оставался ещё в двух
// сторах. routineStore и pinnedStore выставляли error, но его никто не рисовал —
// упавшая операция снова выглядела как «кнопка не работает».
test("ошибка рутин видна в модалке и снимается после успеха", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [], notes: [],
    routines: [{
      id: "r1", title: "Планёрка", days_mask: 3,
      start_mins: 540, duration_mins: 45, active: true,
    }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();
  await page.getByRole("button", { name: "Рутины" }).click();
  await expect(page.locator(".routine-error")).toHaveCount(0);

  await page.evaluate(() => {
    (window as any).__mockFailNext = { cmd: "delete_routine", msg: "Рутина не найдена: r1" };
  });
  await page.getByRole("dialog").getByTitle("Удалить").click();
  await expect(page.locator(".routine-error")).toContainText("Рутина не найдена: r1");

  // успешная операция снимает баннер
  await page.getByRole("dialog").getByTitle("Выключить").click();
  await expect(page.locator(".routine-error")).toHaveCount(0);
});

test("ошибка быстрого слота видна в баннере", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [{
      id: "n1", title: "заметка", content: "текст",
      tags: [], linked_task_id: null, project_id: null, pinned: false,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();

  await page.evaluate(() => {
    (window as any).__mockFailNext = { cmd: "set_pinned_item", msg: "Слот занят другим окном" };
  });
  await page.locator(".note-row").first().getByTitle("В быстрый слот (Ctrl+Shift+J)").click();

  await expect(page.locator(".pinned-error")).toContainText("Слот занят другим окном");
  await page.locator(".pinned-error button").click();
  await expect(page.locator(".pinned-error")).toHaveCount(0);
});

// Пять сторов выставляли error и никогда не сбрасывали его при успехе: единственный
// `error = null` сидел внутри clearError(). Первая же неудача прибивала баннер
// до перезагрузки окна, даже когда всё уже снова работало.
test("ошибка проектов снимается после успешной операции", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Проекты" }).click();
  await page.waitForSelector(".overlay");

  await page.evaluate(() => {
    (window as any).__mockFailNext = { cmd: "create_project", msg: "Проект уже существует" };
  });
  await page.locator("input[placeholder='Название нового проекта']").fill("Первый");
  await page.getByRole("button", { name: "Создать" }).click();
  await expect(page.locator(".alert", { hasText: "Проект уже существует" })).toBeVisible();

  // тот же путь, но без сбоя — баннер обязан исчезнуть
  await page.locator("input[placeholder='Название нового проекта']").fill("Второй");
  await page.getByRole("button", { name: "Создать" }).click();
  await expect(page.locator(".alert", { hasText: "Проект уже существует" })).toHaveCount(0);
});

// v0.9.24: баг из боевой БД — повторяющаяся задача в статусе InProgress
// после клика по ✓ оставалась InProgress на том же месте: визуально ничего
// не происходило, закрыть прогон было невозможно.
test("повторяющаяся задача в работе: ✓ закрывает прогон и возвращает в Todo", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "зарядка");
  await page.locator(".task-main", { hasText: "зарядка" }).dblclick();
  const modal = page.locator(".modal");
  await pickOption(page, modal, "Повтор", "Каждый день");
  await modal.getByRole("button", { name: "Сохранить", exact: true }).click();

  // задача в работе — трекинг запущен, статус InProgress
  await rowMenu(page, "зарядка", "Начать трекинг");
  await expect(page.locator(".track")).toContainText("зарядка");

  // статус виден только на Доске: в Списке его не рендерят — именно поэтому
  // баг и выглядел как «клик по ✓ вообще ничего не делает».
  await page.locator(".seg button", { hasText: "Доска" }).click();
  await expect(page.locator(".column", { hasText: "В работе" })
    .locator(".board-card", { hasText: "зарядка" })).toHaveCount(1);
  await page.locator(".seg button", { hasText: "Список" }).click();

  await page.locator(".task-check").first().click();

  // прогон закрыт: задача осталась в активных (повтор не уходит в историю),
  // вернулась в Todo, таймер остановлен (раньше он продолжал тикать)
  await expect(page.locator(".task-main", { hasText: "зарядка" })).toBeVisible();
  // виджет трекинга исчез — таймер остановлен вместе с прогоном
  await expect(page.locator(".track")).toHaveCount(0);

  await page.locator(".seg button", { hasText: "Доска" }).click();
  await expect(page.locator(".column", { hasText: "Todo" })
    .locator(".board-card", { hasText: "зарядка" })).toHaveCount(1);
  await expect(page.locator(".column", { hasText: "В работе" })
    .locator(".board-card", { hasText: "зарядка" })).toHaveCount(0);
});

test("повтор по дням недели: выбор в модалке сохраняется и отображается индикатором", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  const modal = page.locator(".modal");
  await modal.getByPlaceholder("Название задачи").fill("зарядка");
  await pickOption(page, modal, "Повтор", "По дням недели");

  const dayPicker = modal.locator(".day-picker");
  await expect(dayPicker).toBeVisible();
  await dayPicker.locator(".day-chip", { hasText: "Пн" }).locator("input").check();
  await dayPicker.locator(".day-chip", { hasText: "Ср" }).locator("input").check();
  await dayPicker.locator(".day-chip", { hasText: "Пт" }).locator("input").check();

  await modal.getByRole("button", { name: "Создать" }).click();

  const row = page.locator(".task-row", { hasText: "зарядка" });
  await expect(row.locator(".muted[title*='Пн']")).toHaveCount(1);

  // Редактирование — чекбоксы восстанавливаются из сохранённой маски
  await row.locator(".task-main").dblclick();
  const editModal = page.locator(".modal");
  // Кнопка, а не <select>: значение читается как текст, а не через toHaveValue.
  await expect(editModal.getByRole("button", { name: "Повтор", exact: true }))
    .toContainText("По дням недели");
  const editPicker = editModal.locator(".day-picker");
  await expect(editPicker.locator(".day-chip", { hasText: "Пн" }).locator("input")).toBeChecked();
  await expect(editPicker.locator(".day-chip", { hasText: "Ср" }).locator("input")).toBeChecked();
  await expect(editPicker.locator(".day-chip", { hasText: "Пт" }).locator("input")).toBeChecked();
  await expect(editPicker.locator(".day-chip", { hasText: "Вт" }).locator("input")).not.toBeChecked();
  await editModal.getByRole("button", { name: "Отмена" }).click();
});

test("модалка задач: повтор без выбранных дней недели не сохраняется, дедлайн при повторе не обнуляется", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // «По дням недели» без единого выбранного дня — ошибка, а не тихий откат в «без повтора»
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  const modal = page.locator(".modal");
  await modal.getByPlaceholder("Название задачи").fill("без дней");
  await pickOption(page, modal, "Повтор", "По дням недели");
  await modal.getByRole("button", { name: "Создать" }).click();
  await expect(modal.locator(".alert")).toHaveText("Выберите хотя бы один день недели");
  await modal.getByRole("button", { name: "Отмена" }).click();

  // Дедлайн, указанный вместе с повтором — это время первого срабатывания,
  // и оно должно сохраниться, а не обнулиться (баг до v0.9.21).
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  const modal2 = page.locator(".modal");
  await modal2.getByPlaceholder("Название задачи").fill("полив цветов");
  await modal2.locator('input[type="datetime-local"]').fill("2030-01-15T09:00");
  await pickOption(page, modal2, "Повтор", "Каждый день");
  await expect(modal2.locator(".hint")).toContainText("не закрывается");
  await modal2.getByRole("button", { name: "Создать" }).click();

  const row = page.locator(".task-row", { hasText: "полив цветов" });
  await row.locator(".task-main").dblclick();
  const editModal = page.locator(".modal");
  await expect(editModal.locator('input[type="datetime-local"]')).toHaveValue("2030-01-15T09:00");
  await editModal.getByRole("button", { name: "Отмена" }).click();
});

test("корзина: мягкое удаление, восстановление возвращает в активные, «навсегда» удаляет", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // Список/История/Корзина — один взаимоисключающий переключатель (v0.9.22):
  // .task-list.trash скоупит на панель корзины без лишней xpath-эквилибристики.
  const trashPanel = page.locator(".task-list.trash");

  await createTask(page, "черновик задачи");
  await expect(page.locator(".task-main", { hasText: "черновик задачи" })).toBeVisible();

  await rowMenu(page, "черновик задачи", "Удалить");
  await expect(page.locator(".task-main", { hasText: "черновик задачи" })).toHaveCount(0);

  // В корзине, не в истории
  await page.getByRole("button", { name: "История", exact: true }).click();
  await expect(page.getByText("черновик задачи")).toHaveCount(0);

  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  const trashRow = trashPanel.locator(".task-row", { hasText: "черновик задачи" });
  await expect(trashRow).toBeVisible();

  // Восстановить — снова в активных, из корзины пропадает
  await trashRow.getByRole("button", { name: "Восстановить" }).click();
  await expect(trashPanel.locator(".task-row", { hasText: "черновик задачи" })).toHaveCount(0);
  await page.getByRole("button", { name: "Активные", exact: true }).click();
  await expect(page.locator(".task-main", { hasText: "черновик задачи" })).toBeVisible();

  // Удалить снова, затем стереть навсегда
  await rowMenu(page, "черновик задачи", "Удалить");
  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(trashPanel.locator(".task-row", { hasText: "черновик задачи" })).toBeVisible();
  await trashPanel.locator(".task-row", { hasText: "черновик задачи" }).getByTitle("Удалить навсегда").click();
  await expect(trashPanel.locator(".task-row", { hasText: "черновик задачи" })).toHaveCount(0);

  await page.reload();
  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(page.getByText("Корзина пуста")).toBeVisible();
});

test("список/история/корзина: один взаимоисключающий переключатель, а не два независимых тогла", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "выполненная");
  await createTask(page, "удалённая");
  await page.locator(".task-row").filter({ hasText: "выполненная" }).locator(".task-check").click();
  await rowMenu(page, "удалённая", "Удалить");

  const seg = page.locator(".page-head .seg").nth(1);

  // История и Корзина взаимоисключающи (v0.9.22) — раньше оба можно было
  // открыть одновременно, теперь клик по одному переключает вид целиком.
  await page.getByRole("button", { name: "История", exact: true }).click();
  await expect(seg.getByRole("button", { name: "История", exact: true })).toHaveClass(/active/);
  await expect(page.getByText("выполненная")).toBeVisible();
  await expect(page.locator(".task-list.trash")).toHaveCount(0);

  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(seg.getByRole("button", { name: "Корзина", exact: true })).toHaveClass(/active/);
  await expect(seg.getByRole("button", { name: "История", exact: true })).not.toHaveClass(/active/);
  await expect(page.locator(".task-list.history")).toHaveCount(0);
  await expect(page.getByText("удалённая")).toBeVisible();

  // Иконки визуально различимы: История — зелёная (успех), Корзина — красная
  // (опасность); раньше обе красились в один и тот же зелёный .task-check.done.
  const trashIcon = page.locator(".trash-icon");
  await expect(trashIcon).toHaveCSS("border-color", "rgb(239, 68, 68)");
});

test("история: клик по строке открывает read-only детали с подзадачами и датой завершения", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "поход в горы");
  await page.locator(".chip-sub").click();
  await page.locator(".task-sub-panel .checklist-editor").click();
  await page.keyboard.insertText("рюкзак");
  await page.keyboard.press("Enter");
  await page.keyboard.insertText("палатка");
  await expect(page.locator(".chip-sub")).toHaveText(/0\/2/);
  await page.locator(".task-sub-panel .cm-sub-checkbox").first().click();

  await page.locator(".task-check").click();
  await page.getByRole("button", { name: "История" }).click();

  await page.locator(".history .task-main", { hasText: "поход в горы" }).click();
  const modal = page.locator(".modal");
  await expect(modal.getByText("поход в горы")).toBeVisible();
  await expect(modal.locator(".check-row")).toHaveCount(2);
  // v0.9.24: завершение родителя каскадом закрывает весь чеклист — оба
  // пункта отмечены, хотя вручную был отмечен только первый.
  await expect(modal.locator(".check-row").nth(0).locator("input")).toBeChecked();
  await expect(modal.locator(".check-row").nth(1).locator("input")).toBeChecked();
  await expect(modal.getByText("Завершена")).toBeVisible();

  await modal.getByRole("button", { name: "Закрыть" }).click();
  await expect(page.locator(".modal")).toHaveCount(0);
});

// Отметил последнюю подзадачу — задача выполнена. Правило живёт в бэкенде, а не
// в компоненте чек-листа: подзадачу пишут из модалки, из панели в списке и из
// быстрого слота, и работать должно везде, а не только там, куда пользователь
// нажал. Здесь проверяется, что путь из UI действительно доходит до закрытия.
test("чек-лист: последняя отмеченная подзадача закрывает задачу", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    notes: [],
    settings: { onboarding_complete: true },
    tasks: [{
      id: "t1", title: "задача с чек-листом", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [
        { id: "s1", task_id: "t1", title: "раз", done: false, position: 0 },
        { id: "s2", task_id: "t1", title: "два", done: false, position: 1 },
      ],
      created_at: now, updated_at: now,
    }],
  });
  await withMock(page);
  await page.goto("/");

  // Панель в строке, а не модалка: модалка пишет чек-лист диффом по «Сохранить»,
  // и до нажатия кнопки в БД ничего не меняется. Панель пишет сразу — здесь и
  // видно, что отметка сама доводит задачу до конца.
  await page.waitForSelector(".chip-sub");
  await page.locator(".chip-sub").first().click();
  await page.waitForSelector(".task-sub-panel .checklist-editor", { timeout: 5000 })
    .catch(async () => {
      await page.locator(".chip-sub").first().click();
      await page.waitForSelector(".task-sub-panel .checklist-editor");
    });

  const boxes = page.locator(".task-sub-panel .cm-sub-checkbox");

  // Первая из двух — задача остаётся открытой.
  await boxes.first().click({ force: true });
  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.filter((s: any) => s.done).length
  )).toBe(1);
  expect(await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].status), "закрылась на первой из двух").toBe("Todo");
  // Чип показывает прогресс и НЕ зеленеет, пока сделано не всё (v0.8.2). Проверка
  // переехала сюда из теста жизненного цикла задачи: там она требовала списка из
  // одной подзадачи, а такой теперь закрывает задачу на первой же галочке.
  await expect(page.locator(".chip-sub")).toHaveText(/1\/2/);
  await expect(page.locator(".chip-sub")).not.toHaveClass(/subs-done/);

  // Вторая — закрывает задачу.
  await page.locator(".task-sub-panel .cm-sub-checkbox:not(:checked)").first().click({ force: true });
  await expect.poll(() => page.evaluate(() => {
    const t = JSON.parse(localStorage.getItem("__mock_db")!).tasks[0];
    return [t.status, t.hidden];
  })).toEqual(["Done", true]);

  // И это видно на экране: задача ушла из активных в историю.
  await expect(page.locator(".task-row")).toHaveCount(0);
});

// Повтор: чек-лист — план следующего прогона, поэтому завершение снимает галочки.
// Панель держит свой текст отдельно от стора, и без сброса кэша экран продолжал
// показывать отмеченные квадраты поверх уже очищенной БД — а следующая правка
// записала бы эти галочки обратно, отменив сброс.
test("чек-лист повтора: после автозакрытия галочки снимаются и на экране", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    notes: [],
    settings: { onboarding_complete: true },
    tasks: [{
      id: "t1", title: "ежедневная", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: "Daily", hidden: false, sort_order: 1,
      subtasks: [
        { id: "s1", task_id: "t1", title: "раз", done: false, position: 0 },
        { id: "s2", task_id: "t1", title: "два", done: false, position: 1 },
      ],
      created_at: now, updated_at: now,
    }],
  });
  await withMock(page);
  await page.goto("/");

  await page.waitForSelector(".chip-sub");
  await page.locator(".chip-sub").first().click();
  await page.waitForSelector(".task-sub-panel .checklist-editor", { timeout: 5000 })
    .catch(async () => {
      await page.locator(".chip-sub").first().click();
      await page.waitForSelector(".task-sub-panel .checklist-editor");
    });

  await page.locator(".task-sub-panel .cm-sub-checkbox").first().click({ force: true });
  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.filter((s: any) => s.done).length
  )).toBe(1);

  await page.locator(".task-sub-panel .cm-sub-checkbox:not(:checked)").first().click({ force: true });

  // Повтор не уходит в историю, а едет на следующий дедлайн с чистым чек-листом.
  await expect.poll(() => page.evaluate(() => {
    const t = JSON.parse(localStorage.getItem("__mock_db")!).tasks[0];
    return [t.status, t.hidden, t.subtasks.filter((s: any) => s.done).length];
  })).toEqual(["Todo", false, 0]);

  // И экран это показывает: ни одной отмеченной галочки, счётчик чипа обнулился.
  await expect(page.locator(".task-sub-panel .cm-sub-checkbox:checked")).toHaveCount(0);
  await expect(page.locator(".chip-sub")).toHaveText(/0\/2/);
  await expect(page.locator(".task-row")).toHaveCount(1);
});

// Отмеченная подзадача зачёркивается и уезжает вниз, как в Xiaomi Notes.
// Перенос — только по клику по галочке: это один общий текстовый документ, и
// перестройка на каждую правку уводила бы строку из-под каретки при наборе.
test("чек-лист: отметка зачёркивает подзадачу и уносит её вниз списка", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    notes: [],
    settings: { onboarding_complete: true },
    tasks: [{
      id: "t1", title: "задача с чек-листом", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [
        { id: "s1", task_id: "t1", title: "первая", done: false, position: 0 },
        { id: "s2", task_id: "t1", title: "вторая", done: false, position: 1 },
        { id: "s3", task_id: "t1", title: "третья", done: false, position: 2 },
      ],
      created_at: now, updated_at: now,
    }],
  });
  await withMock(page);
  await page.goto("/");
  await openTaskCard(page, "задача с чек-листом");

  const lines = page.locator(".modal .cm-line");
  await expect(lines).toHaveText(["первая", "вторая", "третья"]);
  await expect(page.locator(".modal .cm-sub-done")).toHaveCount(0);

  // Отмечаем первую — она уходит в конец и зачёркивается.
  await page.locator(".modal .cm-sub-checkbox").first().click();
  await expect(lines).toHaveText(["вторая", "третья", "первая"]);
  const done = page.locator(".modal .cm-sub-done");
  await expect(done).toHaveCount(1);
  await expect(done).toHaveText("первая");
  await expect(done).toHaveCSS("text-decoration-line", "line-through");

  // Отмечена ровно одна галочка, и именно у перенесённой строки. Регресс:
  // виджет сравнивался по {checked, pos}, и после переноса строка, занявшая
  // освободившееся смещение, наследовала чужой DOM-узел с уже переключённым
  // браузером состоянием — на экране оказывались отмечены ОБЕ, при том что
  // текст и зачёркивание были верными.
  await expect(page.locator(".modal .cm-sub-checkbox:checked")).toHaveCount(1);
  const boxes = await page.locator(".modal .cm-line").evaluateAll((ls) =>
    ls.map(l => ({
      text: (l as HTMLElement).innerText,
      checked: (l.querySelector(".cm-sub-checkbox") as HTMLInputElement)?.checked,
    })));
  expect(boxes, "галочка не совпадает со своей строкой").toEqual([
    { text: "вторая", checked: false },
    { text: "третья", checked: false },
    { text: "первая", checked: true },
  ]);

  // Фигура рисуется в SVG, а не средствами родного чекбокса: скобки `[ ]` из
  // границ коробки не собрать. Родной <input> остаётся ради семантики, но
  // спрятан прозрачностью — не display:none, иначе он выпал бы из дерева
  // доступности и состояние перестало бы читаться скринридером.
  const shape = page.locator(".modal .cm-sub-box").first();
  await expect(shape.locator(".cm-sub-bracket")).toHaveCount(2);
  await expect(shape.locator(".cm-sub-tick")).toHaveCount(1);
  await expect(page.locator(".modal .cm-sub-checkbox").first()).toHaveCSS("opacity", "0");

  // Птичка прорисовывается, а не появляется разом. Анимация, а не переход:
  // каждое переключение переписывает весь документ, CodeMirror пересоздаёт
  // виджет, и новый узел приходит уже отмеченным — переходу не от чего идти
  // (замерено: узел до и после клика — разные объекты, промежуточных кадров нет).
  const drawn = await page.locator(".modal .cm-sub-checkbox:checked ~ .cm-sub-svg .cm-sub-tick")
    .evaluate((t) => {
      const anim = (t as SVGElement).getAnimations()[0];
      if (!anim) return null;
      const at = (ms: number) => {
        anim.currentTime = ms;
        return parseFloat(getComputedStyle(t as SVGElement).strokeDashoffset);
      };
      return { start: at(0), mid: at(110), end: at(220) };
    });
  expect(drawn, "у птички нет анимации прорисовки").not.toBeNull();
  expect(drawn!.start).toBeGreaterThan(0);
  expect(drawn!.end).toBeCloseTo(0, 1);
  // Середина строго между концами — то есть линия действительно едет, а не
  // прыгает из скрытого состояния в видимое.
  expect(drawn!.mid).toBeGreaterThan(drawn!.end);
  expect(drawn!.mid).toBeLessThan(drawn!.start);

  // Снятие галочки НЕ тащит строку обратно: порядок после этого — дело рук
  // пользователя, и дёргать список во второй раз незачем.
  await page.locator(".modal .cm-sub-checkbox").last().click();
  await expect(lines).toHaveText(["вторая", "третья", "первая"]);
  await expect(page.locator(".modal .cm-sub-done")).toHaveCount(0);
});

test("модалка: чек-лист подзадач — разметка скрыта чекбоксом, Enter продолжает список, сохранение применяет diff", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // v0.9.45: чеклист — один редактор, а не набор инпутов, и разметка `[x] `
  // пользователю не видна (как в Xiaomi Notes) — вместо неё чекбокс в строке.
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  await page.getByPlaceholder("Название задачи").fill("поездка");
  const editor = page.locator(".modal .checklist-editor");
  await editor.click();
  await page.keyboard.insertText("[ ] паспорт");
  // скобки скрыты виджетом: на экране чекбокс, а не текст разметки
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(1);
  await expect(editor).toHaveText("паспорт");

  // Enter продолжает список — префикс печатать не нужно
  await page.keyboard.press("Enter");
  await page.keyboard.insertText("билеты");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(2);
  // Строка, набранная БЕЗ разметки, — тоже подзадача, и чекбокс у неё есть.
  // Раньше здесь был рассинхрон: parseChecklist считал такую строку подзадачей
  // (нужно для вставки готового списка), а декорация её не рисовала — на
  // экране часть строк выглядела подзадачами, часть просто текстом. Enter
  // после такой строки тоже не продолжал список.
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("без разметки");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await page.keyboard.insertText("вторая");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(2);
  // Отметить можно любую из них, в том числе набранную без разметки:
  // toggleLine дописывает `[x]` такой строке.
  await editor.locator(".cm-sub-checkbox").nth(1).click();
  await expect(editor.locator(".cm-sub-checkbox").nth(1)).toBeChecked();
  await expect(editor.locator(".cm-sub-checkbox").nth(0)).not.toBeChecked();

  // возвращаем исходный чек-лист для проверки diff ниже
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("[ ] паспорт\n[ ] билеты");

  await page.getByRole("button", { name: "Создать" }).click();
  await expect(page.locator(".chip-sub")).toHaveText(/0\/2/);

  // id первой подзадачи до правки — сравниваем с ним же после
  const subsOf = () => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks
      .find((t: any) => t.title === "поездка").subtasks.map((s: any) => [s.id, s.title]));
  const before = await subsOf();
  expect(before.map((s: string[]) => s[1])).toEqual(["паспорт", "билеты"]);

  // редактирование: правка формулировки + удаление строки
  await page.locator(".task-main", { hasText: "поездка" }).dblclick();
  await expect(editor).toHaveText("паспортбилеты"); // две строки без разметки
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("[ ] загранпаспорт");
  // exact — иначе матчится и «Сохранить как шаблон» в авто-развёрнутой панели
  await page.locator(".modal").getByRole("button", { name: "Сохранить", exact: true }).click();

  await expect(page.locator(".chip-sub")).toHaveText(/0\/1/);
  // Правка формулировки — переименование, а не пересоздание: id тот же самый.
  // Если бы diff пересоздавал подзадачу, отметка «выполнено» слетала бы при
  // каждой правке текста.
  expect(await subsOf()).toEqual([[before[0][0], "загранпаспорт"]]);
});

test("композер: Shift+Enter — подзадачи, Ctrl+Enter — создать", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".composer-input").click();
  await page.keyboard.type("быстрая задача");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("шаг раз");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("шаг два");
  await page.keyboard.press("Control+Enter");

  // задача в списке, две подзадачи в чипе, композер очищен
  await expect(page.locator(".task-main", { hasText: "быстрая задача" })).toBeVisible();
  await expect(page.locator(".chip-sub")).toHaveText(/0\/2/);
  await expect(page.locator(".composer-input")).toHaveValue("");

  // v0.8.3: панель авто-развёрнута; v0.9.45: чек-лист текстом, разметка скрыта
  const panel = page.locator(".task-sub-panel .checklist-editor");
  await expect(panel.locator(".cm-sub-checkbox")).toHaveCount(2);
  await expect(panel).toHaveText("шаг разшаг два");
});

// v0.9.45: панель в строке задачи получила тот же чек-лист-редактор, что
// модалка и быстрый слот. Здесь запись мгновенная (через паузу набора), а diff
// позиционный — правка формулировки обязана остаться переименованием, иначе на
// каждой букве подзадача пересоздавалась бы и теряла отметку «выполнено».
test("панель задачи: правка подзадачи — переименование, отметка не слетает", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "собрать цифры", done: true, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
  });
  await withMock(page);
  await page.goto("/");

  const panel = page.locator(".task-sub-panel .checklist-editor");
  await expect(panel.locator(".cm-sub-checkbox")).toBeChecked();

  // правим формулировку, отметку не трогаем
  await panel.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("[x] собрать цифры за квартал");

  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks
      .map((s: any) => [s.id, s.title, s.done]))
  ).toEqual([["s1", "собрать цифры за квартал", true]]);
});

test("композер: двойное нажатие Enter не создаёт дубликат", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".composer-input").click();
  await page.keyboard.type("одна задача");
  // Два быстрых Enter — должен сработать только первый
  await page.keyboard.press("Control+Enter");
  await page.keyboard.press("Control+Enter");

  await expect(page.locator(".task-main", { hasText: "одна задача" })).toHaveCount(1);
  await expect(page.locator(".composer-input")).toHaveValue("");
});

test("календарь: клик по дню создаёт задачу с дедлайном этого дня", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Календарь" }).click();
  await page.locator(".day.today").click();

  await expect(page.getByText("Новая задача")).toBeVisible();
  // дедлайн предзаполнен на 09:00 выбранного дня
  const deadline = await page.locator('input[type="datetime-local"]').inputValue();
  expect(deadline).toMatch(/^\d{4}-\d{2}-\d{2}T09:00$/);

  await page.getByPlaceholder("Название задачи").fill("задача из календаря");
  await page.getByRole("button", { name: "Создать" }).click();

  await expect(page.locator(".day.today .task-chip", { hasText: "задача из календаря" })).toBeVisible();
});

test("заметки: чек-лист рендерится инлайн (live preview) и переключается кликом", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();

  await fillNoteEditor(page, "план:\n- [ ] первый пункт\n- [ ] второй пункт");

  const boxes = page.locator(".cm-task-checkbox");
  await expect(boxes).toHaveCount(2);
  await expect(boxes.first()).not.toBeChecked();
  await boxes.first().click();
  await expect(boxes.first()).toBeChecked();
  await page.waitForTimeout(900); // дебаунс автосохранения (800мс)

  // клик переписывает markdown-источник, а не только DOM-виджет: перечитываем
  // заметку с нуля (reload → перечитать заметки из "БД") — если бы правился
  // только чекбокс в DOM, а не editContent, состояние бы потерялось.
  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item").first().click();
  await expect(page.locator(".cm-task-checkbox").first()).toBeChecked();
});

test("редактор: **жирный** внутри ```кода``` не рендерится жирным, снаружи — рендерится", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  const editor = noteEditor(page);

  await fillNoteEditor(page, "до\n\n```\n**код**\n```\n\n**снаружи**");

  // Уводим курсор на первую строку, чтобы **снаружи** не был сырым
  await editor.click();
  await page.keyboard.press("ControlOrMeta+Home");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");

  await expect(page.locator(".cm-strong")).toHaveCount(1);
});

// v0.9.27: декорации на Lezer-дереве — цитаты и нумерованные списки
// многострочны и вкладываются, построчным regex их разбирать нельзя.
test("редактор: цитаты и нумерованные списки декорируются по Lezer-дереву", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  const editor = noteEditor(page);

  await fillNoteEditor(page, "> первая строка цитаты\n> вторая строка\n\n1. раз\n2. два\n3. три");

  // Уводим курсор в конец, чтобы маркеры '>' не показывались как сырые
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");

  // обе строки цитаты получили класс, а не только первая
  await expect(page.locator(".cm-quote")).toHaveCount(2);
  // каждый пункт нумерованного списка — своя строка
  await expect(page.locator(".cm-ol-item")).toHaveCount(3);
  // номера НЕ прячутся: цифра — часть текста, её правит пользователь
  await expect(editor).toContainText("1. раз");
  await expect(editor).toContainText("3. три");
});

test("редактор: цитата внутри цитаты не ломает разметку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  const editor = noteEditor(page);

  await fillNoteEditor(page, "> внешняя\n> > вложенная\n> снова внешняя");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");

  // все три строки — часть цитаты (вложенность обрабатывается деревом)
  await expect(page.locator(".cm-quote")).toHaveCount(3);
});

// v0.9.29: справка в Настройках — свёрнутые темы на <details>.
test("справка: темы раскрываются, поиск по настройкам находит внутри свёрнутых", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Справка" }).click();

  // темы видны, содержимое свёрнуто
  const notesTopic = page.locator(".help-topic", { hasText: "Заметки" }).first();
  await expect(notesTopic).toBeVisible();
  await expect(notesTopic.locator("dd").first()).not.toBeVisible();

  // клик раскрывает
  await notesTopic.locator("summary").click();
  await expect(notesTopic.locator("dd").first()).toBeVisible();

  // Ключевое: свёрнутый <details> оставляет текст в DOM, поэтому поиск по
  // настройкам (читает textContent) находит его и раскрывает темы.
  await page.locator(".settings-tab", { hasText: "Общее" }).click();
  await page.getByPlaceholder(/Поиск/).fill("Автолинковка");
  await expect(page.getByText("Предлагает вики-ссылки на другие заметки по смыслу текста.")).toBeVisible();
});

// v0.9.29: Онбординг остаётся коротким и ссылается на справку.
test("онбординг: последний шаг ведёт в Настройки → Справка", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: false } });
  await withMock(page);
  await page.goto("/");

  // v0.10.14: первым идёт выбор языка.
  await page.getByRole("button", { name: "Далее" }).click();
  await page.getByRole("button", { name: "Начать настройку" }).click();
  await page.getByRole("button", { name: "Далее" }).click();
  await page.getByRole("button", { name: "Далее" }).click();
  // v0.9.64: между автозагрузкой и финалом появился шаг голосового ввода.
  await page.getByRole("button", { name: "Далее" }).click();
  await expect(page.getByText("Готово!")).toBeVisible();
  await expect(page.getByText("Настройках → Справка")).toBeVisible();
});

// v0.9.64: голосовой ввод требует отдельной модели, поэтому у него свой шаг в
// онбординге — но приложение обязано работать и без неё, так что шаг проходится
// насквозь без единого скачивания.
test("онбординг: шаг голосового ввода пропускается без скачивания", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: false } });
  await withMock(page);
  await page.goto("/");

  // v0.10.14: первым идёт выбор языка.
  await page.getByRole("button", { name: "Далее" }).click();
  await page.getByRole("button", { name: "Начать настройку" }).click();
  await page.getByRole("button", { name: "Далее" }).click();
  await page.getByRole("button", { name: "Далее" }).click();

  await expect(page.getByText("Голосовой ввод")).toBeVisible();
  // именно whisper-каталог, а не список чат-моделей
  await expect(page.getByText("Whisper Base")).toBeVisible();
  await expect(page.getByText("Qwen2.5 1.5B Instruct")).toHaveCount(0);

  // шаг необязателен: «Далее» доводит до финала, ничего не скачав
  await page.getByRole("button", { name: "Далее" }).click();
  await expect(page.getByText("Готово!")).toBeVisible();
});

// v0.9.64: у распознавания свой раздел, не зависящий от ai_provider — модель
// нужна и тогда, когда чат-модель облачная. Раньше единственный загрузчик жил
// внутри блока «локальная модель» и при облачном провайдере не существовал.
test("настройки: раздел голосового ввода есть и при облачном ИИ", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true, ai_provider: "openai" } });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab").getByText("ИИ", { exact: true }).click();

  await expect(page.getByText("Голосовой ввод")).toBeVisible();
  // путь и каталог — whisper'овские, а не от чат-модели
  await expect(page.getByText("/home/user/.local/share/com.kriptag.app/models/whisper.bin")).toBeVisible();
  await expect(page.getByText("Whisper Base")).toBeVisible();
});

// v0.9.28: путь к модели приходит от бэкенда (app_data_dir зависит от ОС),
// а не собирается строкой в UI. Раньше был зашит
// `~/.local/share/kriptag/models/model.gguf` — неверный на Windows/macOS и
// неверный даже на Linux (каталог называется по identifier'у приложения).
test("настройки: путь к локальной модели берётся из бэкенда, а не зашит в UI", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true, ai_provider: "local" } });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  // exact: «ИИ» подстрокой матчит и «Уведомления» — strict mode violation
  await page.locator(".settings-tab").getByText("ИИ", { exact: true }).click();
  await expect(page.getByText("/home/user/.local/share/com.kriptag.app/models/model.gguf")).toBeVisible();
  // старая зашитая строка не должна остаться нигде
  await expect(page.getByText("~/.local/share/kriptag/models")).toHaveCount(0);
});

// v0.9.28: совет про композитор специфичен для Wayland — на Windows его быть
// не должно (isWayland уже прокидывался в компонент, но не использовался).
test("онбординг: совет про Hyprland/Sway показывается только на Wayland", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: false } });
  await withMock(page);
  await page.goto("/"); // мок отдаёт is_wayland: false — шаг Wayland пропускается

  // проходим до шага автозагрузки (шаг Wayland пропущен — is_wayland: false)
  await page.getByRole("button", { name: "Далее" }).click();
  await page.getByRole("button", { name: "Начать настройку" }).click();
  await page.getByRole("button", { name: "Далее" }).click();
  await expect(page.getByText("Автозагрузка и хоткеи")).toBeVisible();

  // не-Wayland: ни отсылки к композитору, ни отдельного шага с конфигом —
  // хоткеи там регистрирует само приложение (v0.10.14).
  await expect(page.getByText("композитор")).toHaveCount(0);
  await expect(page.getByText("Быстрый ввод из любого места")).toBeVisible();
  await page.getByRole("button", { name: "Далее" }).click();
  await expect(page.getByText("Хоткеи в композиторе"), "шаг биндов показан не на Wayland").toHaveCount(0);
});

// v0.9.27: кнопки на панели форматирования для тех же трёх конструкций.
test("панель: кнопки цитаты, нумерованного списка и ссылки", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  const editor = noteEditor(page);

  // Цитата — префикс на текущей строке, повторный клик снимает.
  // fillNoteEditor паркует курсор на пустой строке НИЖЕ текста, поэтому
  // возвращаем его на саму строку — префикс ставится по строке курсора.
  await fillNoteEditor(page, "мысль");
  await page.keyboard.press("ControlOrMeta+Home");
  await page.getByTitle("Цитата").click();
  await expect(editor).toContainText("> мысль");
  await page.getByTitle("Цитата").click();
  await expect(editor).not.toContainText("> мысль");

  // Нумерованный список: выделяем три строки — нумерация идёт подряд,
  // а не одинаковым префиксом на всех
  await fillNoteEditor(page, "раз\nдва\nтри");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.getByTitle("Нумерованный список").click();
  await expect(editor).toContainText("1. раз");
  await expect(editor).toContainText("2. два");
  await expect(editor).toContainText("3. три");

  // Ссылка из выделения: текст становится подписью, курсор — внутри скобок,
  // поэтому url допечатывается сразу
  await fillNoteEditor(page, "документация");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.getByTitle("Ссылка", { exact: true }).click();
  await page.keyboard.type("https://example.com");
  await expect(editor).toContainText("[документация](https://example.com)");
});

// v0.9.27: обычные [текст](url) раньше не декорировались вообще.
test("редактор: markdown-ссылка рендерится, опасная схема блокируется", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  const editor = noteEditor(page);

  // Пустая строка в конце: курсор паркуется там, чтобы ни одна строка со
  // ссылкой не была «сырой» (на строке с курсором декорации не применяются).
  await fillNoteEditor(page, "[документация](https://example.com/docs)\n\n[плохая](javascript:void)\n\nконец");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");

  // нормальная ссылка — виджет с текстом, без markdown-синтаксиса вокруг
  const good = page.locator("a.cm-mdlink", { hasText: "документация" });
  await expect(good).toBeVisible();
  await expect(good).not.toHaveClass(/unsafe/);

  // javascript: помечена как заблокированная, а не открывается молча
  await expect(page.locator("a.cm-mdlink.unsafe", { hasText: "плохая" })).toBeVisible();
});

test("вики-заметки: автодополнение, [[ссылка]] открывает/создаёт, бэклинки, поиск", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  const title = page.getByPlaceholder("Название", { exact: true });
  const editor = noteEditor(page);

  // заметка-цель
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Идея");

  // вторая заметка: автодополнение по "[[" (штатный автокомплит CodeMirror)
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Черновик");
  await editor.click();
  await page.keyboard.type("См. [[Ид");
  await expect(page.locator(".cm-tooltip-autocomplete", { hasText: "Идея" })).toBeVisible();
  // Тултип уже виден, но CM применяет его на следующий кадр — без этого Enter
  // иногда успевает вставить перевод строки раньше, чем completion активна.
  await page.waitForTimeout(150);
  await page.keyboard.press("Enter");
  await expect(editor).toContainText("См. [[Идея]]");

  // живая ссылка + битая (dashed) — рендерятся сразу, без отдельного режима
  await fillNoteEditor(page, "См. [[Идея]] и [[Новая мысль]]");
  const good = page.locator("a.cm-wikilink", { hasText: "Идея" });
  await expect(good).toBeVisible();
  await expect(page.locator("a.cm-wikilink.missing", { hasText: "Новая мысль" })).toBeVisible();

  // клик открывает целевую заметку; бэклинк ведёт обратно
  await good.click();
  await expect(title).toHaveValue("Идея");
  const backlink = page.locator(".backlink", { hasText: "Черновик" });
  await expect(backlink).toBeVisible();
  await backlink.click();
  await expect(title).toHaveValue("Черновик");

  // клик по битой ссылке создаёт заметку с этим названием
  await page.locator("a.cm-wikilink.missing", { hasText: "Новая мысль" }).click();
  await expect(title).toHaveValue("Новая мысль");

  // Ctrl+K находит заметку по содержимому (search_notes)
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Поиск задач и заметок...").fill("Идея]] и");
  await page.locator(".result", { hasText: "Черновик" }).click();
  await expect(title).toHaveValue("Черновик");
});

test("вики-заметки: переименование обновляет ссылки в других заметках", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  const title = page.getByPlaceholder("Название", { exact: true });
  const editor = noteEditor(page);

  // целевая заметка
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Идея");
  await page.waitForTimeout(900); // дебаунс автосохранения (800мс)

  // заметка со ссылкой (простой + с алиасом) на неё
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Черновик");
  await fillNoteEditor(page, "см. [[Идея]] и [[Идея|та самая]]");
  await page.waitForTimeout(900);

  // переименовываем целевую — тост появляется, ссылки в «Черновике» обновились
  await page.locator(".note-item", { hasText: "Идея" }).click();
  await title.fill("Идея v2");
  await expect(page.locator(".rename-toast")).toHaveText("Обновлено ссылок: 1");

  // ссылки отрендерены живьём (виджеты, не сырой текст): цель и алиас — как надо
  await page.locator(".note-item", { hasText: "Черновик" }).click();
  await expect(page.locator("a.cm-wikilink", { hasText: "Идея v2" })).toHaveCount(1);
  await expect(page.locator("a.cm-wikilink", { hasText: "та самая" })).toHaveCount(1);

  // клик по обновлённой ссылке всё ещё открывает ту же заметку
  await page.locator("a.cm-wikilink", { hasText: "Идея v2" }).first().click();
  await expect(title).toHaveValue("Идея v2");
});

test("ИИ-автолинковка: кнопка скрыта без ИИ, с ИИ предлагает связи, принятие вставляет [[ссылку]]", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  const title = page.getByPlaceholder("Название", { exact: true });
  const editor = noteEditor(page);

  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Соседняя");
  await page.waitForTimeout(900);

  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Главная");
  await editor.click();
  await page.keyboard.type("текст без ссылок");
  await page.waitForTimeout(900);

  // без ИИ пункта нет вовсе — проверяем внутри меню, куда действие переехало
  await page.getByRole("button", { name: "Действия с заметкой" }).click();
  await expect(page.locator('[role="menuitem"]', { hasText: "ИИ предложит заметки" }))
    .toHaveCount(0);
  await page.keyboard.press("Escape");

  // включаем ИИ (in-place, сохраняя уже созданные заметки) и перезаходим,
  // чтобы капабилити-детект перечитал настройки
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.settings.ai_provider = "local";
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "Главная" }).click();

  await noteAction(page, "ИИ предложит заметки для связи");

  const chip = page.locator(".link-chip", { hasText: "Соседняя" });
  await expect(chip).toBeVisible();
  await chip.click();
  // вставленная ссылка на новой строке — курсор туда не переходит (текст
  // меняется программно), поэтому строка рендерится живьём, как виджет
  await expect(page.locator("a.cm-wikilink", { hasText: "Соседняя" })).toBeVisible();
  // принятая связь пропадает из списка предложений
  await expect(page.locator(".link-chip", { hasText: "Соседняя" })).toHaveCount(0);
});

test("редактор заметок: переключение между заметками не портит undo-историю", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  const title = page.getByPlaceholder("Название", { exact: true });
  const editor = noteEditor(page);

  // Заметка А
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Заметка А");
  // Ждём сохранение (автосейв 800 мс + запас)
  await page.waitForTimeout(1000);
  await fillNoteEditor(page, "Содержимое А");

  // Заметка Б
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await title.fill("Заметка Б");
  await page.waitForTimeout(1000);
  await fillNoteEditor(page, "Содержимое Б");

  // Возвращаемся к А, потом снова к Б
  await page.locator(".note-item", { hasText: "Заметка А" }).click();
  await page.waitForTimeout(500);
  await page.locator(".note-item", { hasText: "Заметка Б" }).click();
  await page.waitForTimeout(500);

  // Ctrl+Z в Б — история чистая, содержимое не должно измениться
  await editor.click();
  await page.keyboard.press("ControlOrMeta+z");
  await page.waitForTimeout(300);

  // Содержимое Б — всё ещё "Содержимое Б"
  await expect(editor).toContainText("Содержимое Б");
});

test("Ctrl+K находит задачу и открывает раздел задач", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "искомая задача");
  // уходим в другой раздел, чтобы проверить навигацию из поиска
  await page.getByRole("button", { name: "Дашборд" }).click();

  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Поиск задач и заметок...").fill("искомая");
  await page.locator(".result", { hasText: "искомая задача" }).click();

  await expect(page.getByRole("heading", { name: "Задачи" })).toBeVisible();
  await expect(page.locator(".task-main", { hasText: "искомая задача" })).toBeVisible();
});

test("командная палитра: клавиатурная навигация и фильтр по вводу", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // Стрелка вниз/Enter по действию «Новая заметка» → раздел заметок
  await page.keyboard.press("Control+k");
  await page.locator(".result", { hasText: "Новая задача" }).waitFor();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".result.active")).toHaveText(/Новая заметка/);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("button", { name: "+ Новая заметка" })).toBeVisible();

  // Ввод «дашб» фильтрует действия до «Перейти: Дашборд»
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Поиск задач и заметок...").fill("дашб");
  await expect(page.locator(".result")).toHaveCount(1);
  await expect(page.locator(".result")).toHaveText(/Дашборд/);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "Дашборд" })).toBeVisible();
});

test("командная палитра: «Спланировать день» переходит в календарь-неделю", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Поиск задач и заметок...").fill("спланировать");
  await page.locator(".result", { hasText: "Спланировать день" }).click();

  await expect(page.getByRole("heading", { name: "Календарь" })).toBeVisible();
  // v0.9.54: выбранный режим помечается .active в общем .seg (был .active-toggle)
  await expect(page.locator(".seg button.active", { hasText: "Неделя" })).toBeVisible();
});

test("командная палитра: «Сменить тему» переключает и сохраняет тему", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true, theme_mode: "light" } });
  await withMock(page);
  await page.goto("/");

  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Поиск задач и заметок...").fill("сменить тем");
  await page.locator(".result", { hasText: "Сменить тему" }).click();

  const db = JSON.parse(await page.evaluate(() => localStorage.getItem("__mock_db")!));
  expect(db.settings.theme_mode).toBe("dark");
});

test("проекты: модалка центрирована, не растянута на весь экран", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // Открываем модалку проектов
  await page.getByRole("button", { name: "Проекты" }).click();
  await page.waitForSelector(".overlay");

  const vp = page.viewportSize();
  const modalBox = await page.locator(".modal.dialog").boundingBox();
  expect(modalBox).not.toBeNull();
  if (modalBox && vp) {
    // Высота модалки меньше 90% высоты вьюпорта
    expect(modalBox.height).toBeLessThan(vp.height * 0.9);
    // Модалка центрирована по горизонтали (слева меньше половины ширины вьюпорта)
    expect(modalBox.x).toBeGreaterThan(0);
    expect(modalBox.x + modalBox.width).toBeLessThan(vp.width);
  }
});

test("проекты: создание, назначение задаче, группировка и фильтр", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // создать проект
  await page.getByRole("button", { name: "Проекты" }).click();
  await page.getByPlaceholder("Название нового проекта").fill("Ремонт");
  await page.getByRole("button", { name: "Создать" }).click();
  // Именно кнопка модалки: с v0.9.40 «Закрыть» есть ещё и у кнопок окна.
  await page.locator(".modal").getByRole("button", { name: "Закрыть" }).click();

  // задача в проект через модал
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  await page.getByPlaceholder("Название задачи").fill("покрасить стены");
  await pickOption(page, page, "Проект", "Ремонт");
  await page.getByRole("button", { name: "Создать" }).click();
  await createTask(page, "задача вне проекта");

  // группировка: заголовки секций видны
  await expect(page.locator(".project-head", { hasText: "Ремонт" })).toBeVisible();
  await expect(page.locator(".project-head", { hasText: "Без проекта" })).toBeVisible();

  // фильтр по проекту
  await page.locator(".project-filter").selectOption({ label: "Ремонт" });
  await expect(page.getByText("покрасить стены")).toBeVisible();
  await expect(page.locator(".task-main", { hasText: "задача вне проекта" })).toHaveCount(0);
});

test("цель проекта: прогресс в заголовке группы, зелёная при выполнении, карта на дашборде", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // проект с целью «1 задача в неделю»
  await page.getByRole("button", { name: "Проекты" }).click();
  await page.getByPlaceholder("Название нового проекта").fill("Спорт");
  await page.getByRole("button", { name: "Создать" }).click();
  await page.locator(".proj-goal .goal-num").first().fill("1");
  await page.locator(".proj-goal .goal-num").first().blur();
  // чип прогресса появился в модале
  await expect(page.locator(".proj-goal .goal-chip")).toHaveText("0/1 задач");
  // см. выше: кнопка модалки, не кнопка окна
  await page.locator(".modal").getByRole("button", { name: "Закрыть" }).click();

  // задача в проекте → в заголовке группы виден прогресс цели
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  await page.getByPlaceholder("Название задачи").fill("пробежка");
  await pickOption(page, page, "Проект", "Спорт");
  await page.getByRole("button", { name: "Создать" }).click();
  const headChip = page.locator(".project-head .goal-chip");
  await expect(headChip).toHaveText("0/1 задач");
  await expect(headChip).not.toHaveClass(/met/);

  // выполнение задачи закрывает цель — чип зеленеет
  await page.locator(".task-check").click();
  await expect(page.locator(".project-head")).toHaveCount(0); // группа опустела
  await page.getByRole("button", { name: "Проекты" }).click();
  await expect(page.locator(".proj-goal .goal-chip")).toHaveText("1/1 задач");
  await expect(page.locator(".proj-goal .goal-chip")).toHaveClass(/met/);
  // см. выше: кнопка модалки, не кнопка окна
  await page.locator(".modal").getByRole("button", { name: "Закрыть" }).click();

  // карта «Цели проектов» на дашборде
  await page.getByRole("button", { name: "Дашборд" }).click();
  await expect(page.getByText("Цели проектов")).toBeVisible();
  const goalCard = page.locator(".goal-item", { hasText: "Спорт" });
  await expect(goalCard).toBeVisible();
  await expect(goalCard.locator(".goal-val")).toHaveText("1/1");
});

// v0.9.32: язык интерфейса. Единственный тест, работающий с английским —
// остальные прибиты к русскому в моке (иначе каждая строка словаря ломала
// бы десятки тестов, проверяющих логику, а не перевод).
test("язык: переключение меняет интерфейс сразу и переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // Навигация по-русски: мок отдаёт language: "ru"
  await expect(page.locator(".nav-item span", { hasText: "Задачи" })).toBeVisible();

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // Применяется сразу, без «Сохранить» — как и тема
  await expect(page.locator(".nav-item span", { hasText: "Tasks" })).toBeVisible();
  await expect(page.locator(".nav-item span", { hasText: "Задачи" })).toHaveCount(0);

  // Автосохранение переживает перезагрузку. Индикатор на английском — язык уже
  // переключён этим же тестом.
  await expect(page.locator(".autosave-note")).toContainText(/Сохранено|Saved/, { timeout: 5000 });
  await page.reload();
  await expect(page.locator(".nav-item span", { hasText: "Tasks" })).toBeVisible();

  // и обратно
  await page.getByRole("button", { name: "Settings" }).click();
  await pickSetting(page, "Language", "Русский");
  await expect(page.locator(".nav-item span", { hasText: "Задачи" })).toBeVisible();
});

// Непереведённая строка должна деградировать в русский оригинал, а не в
// пустоту или ключ — это главное свойство схемы «ключ = русский текст».
test("язык: строки без перевода остаются русскими, а не пустыми", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // «Внешний вид» переведён, а заголовки секций ниже — ещё нет; они должны
  // остаться читаемыми русскими, а не превратиться в пустые блоки.
  await expect(page.locator(".section-title", { hasText: "Appearance" })).toBeVisible();
  const titles = await page.locator(".section-title").allTextContents();
  expect(titles.every(s => s.trim().length > 0)).toBe(true);
});

// v0.9.31: домены в трекинге. Приватностная фича — тест проверяет прежде
// всего, что по умолчанию ничего не собирается и не показывается.
test("домены: выключены по умолчанию, галочка сохраняется, историю можно забыть", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  // windowTracking пишем ПОСЛЕ загрузки, а не через seedDb: init-скрипт
  // seedDb выполняется заново на каждый reload и затирает всё, что мок
  // успел сохранить — а этот тест как раз проверяет переживание reload.
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.windowTracking = "hyprland";
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  await page.getByRole("button", { name: "Настройки" }).click();
  // Мониторинг живёт на вкладке «Категории» (SECTION_TAB[3])
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  const toggle = page.getByText("Разбивать браузерное время по сайтам");
  await expect(toggle).toBeVisible();
  const cb = page.locator("label", { hasText: "Разбивать браузерное время" }).locator("input[type=checkbox]");
  await expect(cb).not.toBeChecked(); // выкл по умолчанию

  // Явно сказано, что заголовок не сохраняется — формулировка часть фичи
  await expect(page.getByText(/только домен/i)).toBeVisible();

  await flipSwitch(page, "Разбивать браузерное время");
  await expect(cb).toBeChecked();
  await waitSettingsSaved(page);
  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  await expect(page.locator("label", { hasText: "Разбивать браузерное время" })
    .locator("input[type=checkbox]")).toBeChecked();

  // Кнопка забывания собранного отвечает числом, а не молчит
  await page.getByRole("button", { name: "Забыть собранные домены" }).click();
  await expect(page.getByText(/Очищено записей/)).toBeVisible();
});

test("домены: блок «Сайты» на дашборде появляется только когда есть данные", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  // Не через seedDb: его init-скрипт перезапускается на reload и вернул бы
  // domainUsage обратно, а вторая половина теста как раз проверяет пустой случай.
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.windowTracking = "hyprland";
    db.domainUsage = [{ domain: "github.com", minutes: 42 }];
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Дашборд" }).click();
  // .section-title: слово «Сайты» встречается ещё и в тексте Справки (v0.9.29)
  await expect(page.locator(".section-title", { hasText: "Сайты" })).toBeVisible();
  await expect(page.getByText("github.com")).toBeVisible();

  // Без данных блока нет вовсе — пустой заголовок «Сайты» читался бы как
  // поломка, а не как выключенная функция.
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.domainUsage = [];
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Дашборд" }).click();
  await expect(page.locator(".section-title", { hasText: "Сайты" })).toHaveCount(0);
});

// v0.9.30: простой внутри тайм-блока по данным мониторинга — «план vs факт».
test("тайм-блок: простой из мониторинга виден на блоке, ноль не показывается", async ({ page }) => {
  const now = new Date();
  const iso = (h: number) =>
    new Date(now.getFullYear(), now.getMonth(), now.getDate(), h, 0).toISOString();
  const dayKey = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;

  await seedDb(page, {
    tasks: [
      { id: "b1", title: "с простоем", status: "Todo", priority: "Medium", category: "Other",
        tags: [], description: null, deadline: null, recurrence: "None", hidden: false,
        project_id: null, scheduled_at: iso(10), scheduled_mins: 60, sort_order: 1,
        subtasks: [], created_at: iso(9), updated_at: iso(9), completed_at: null },
      { id: "b2", title: "без простоя", status: "Todo", priority: "Medium", category: "Other",
        tags: [], description: null, deadline: null, recurrence: "None", hidden: false,
        project_id: null, scheduled_at: iso(14), scheduled_mins: 60, sort_order: 2,
        subtasks: [], created_at: iso(9), updated_at: iso(9), completed_at: null },
    ],
    notes: [],
    // Мониторинг вернул простой только для первого блока
    blockIdle: { [dayKey]: [{ task_id: "b1", task_title: "с простоем", planned_mins: 60, idle_mins: 25, active_mins: 35 }] },
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();

  // Блок с простоем — подпись есть
  await expect(page.locator(".block", { hasText: "с простоем" }).locator(".block-idle"))
    .toHaveText(/простой 25 мин/);

  // Блок без данных — подписи нет вовсе (а не «0 мин»): у будущего блока
  // простоя нет по определению, и ноль читался бы как утверждение о факте.
  await expect(page.locator(".block", { hasText: "без простоя" }).locator(".block-idle"))
    .toHaveCount(0);
});

test("тайм-блокинг: drag из бэклога ставит блок, задача видна в «Сегодня»", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "глубокая работа");

  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();

  const backlogItem = page.locator(".backlog-item", { hasText: "глубокая работа" });
  await expect(backlogItem).toBeVisible();
  // ИИ выключен (дефолт мока) — планировщик скрыт (капабилити-детект)
  await expect(page.getByRole("button", { name: "Спланировать день" })).toHaveCount(0);

  // бросаем на колонку сегодняшнего дня (~середина утра)
  await backlogItem.dragTo(page.locator(".week-col.today"), { targetPosition: { x: 40, y: 400 } });

  const block = page.locator(".block", { hasText: "глубокая работа" });
  await expect(block).toBeVisible();
  await expect(backlogItem).toHaveCount(0); // из бэклога ушла
  await expect(block.locator(".block-time")).toHaveText(/\d{2}:\d{2}–\d{2}:\d{2}/);

  // строка «Сегодня:» в разделе задач
  await page.getByRole("button", { name: "Задачи" }).click();
  await expect(page.locator(".day-plan-chip", { hasText: "глубокая работа" })).toBeVisible();

  // снять блок — вернулась в бэклог
  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();
  await page.locator(".block", { hasText: "глубокая работа" }).hover();
  await page.locator(".block-x").click();
  await expect(page.locator(".backlog-item", { hasText: "глубокая работа" })).toBeVisible();
});

test("ИИ-планировщик: план дня — призрак → применить → блок; «Что сейчас?» — совет", async ({ page }) => {
  // капабилити-детект: кнопки планировщика видны только при включённом ИИ
  await seedDb(page, { tasks: [], notes: [], settings: { ai_provider: "local" } });
  await withMock(page);
  await page.goto("/");

  await createTask(page, "важное дело");

  // «Что сейчас?» — совет баннером
  await page.getByRole("button", { name: "Что сейчас?" }).click();
  await expect(page.locator(".what-now")).toContainText("Совет мока");
  await page.locator(".what-now .btn-icon").click();
  await expect(page.locator(".what-now")).toHaveCount(0);

  // План дня: призрак в сетке, применение ставит настоящий блок
  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();
  await page.getByRole("button", { name: "Спланировать день" }).click();

  const ghost = page.locator(".block.ghost", { hasText: "важное дело" });
  await expect(ghost).toBeVisible();
  await expect(page.locator(".backlog-item", { hasText: "важное дело" })).toBeVisible(); // ещё в бэклоге

  await page.getByRole("button", { name: "Применить" }).click();
  await expect(page.locator(".block.ghost")).toHaveCount(0);
  await expect(page.locator(".block", { hasText: "важное дело" })).toBeVisible();
  await expect(page.locator(".block .block-time", { hasText: "10:00–11:00" })).toBeVisible();
  await expect(page.locator(".backlog-item", { hasText: "важное дело" })).toHaveCount(0);
});

test("помодоро: виджет виден при активной фазе, пауза/продолжить и пропуск фазы", async ({ page }) => {
  const until = new Date(Date.now() + 12 * 60 * 1000).toISOString();
  await seedDb(page, {
    tasks: [], notes: [], settings: { onboarding_complete: true },
    pomodoro: { phase: "work", until },
  });
  await withMock(page);
  await page.goto("/");

  const widget = page.locator(".pomo");
  await expect(widget).toBeVisible();
  await expect(widget.locator(".pomo-label")).toHaveText("Фокус");

  await widget.getByTitle("Пауза").click();
  await expect(widget.locator(".pomo-label")).toHaveText("Пауза");

  await widget.getByTitle("Продолжить").click();
  await expect(widget.locator(".pomo-label")).toHaveText("Фокус");

  await widget.getByTitle("Пропустить фазу").click();
  await expect(widget.locator(".pomo-label")).toHaveText("Перерыв");
});

test("помодоро: ▶ на виджете при off запускает ручной цикл, ■ останавливает", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  const widget = page.locator(".pomo");
  await expect(widget.getByTitle("Начать помидор")).toBeVisible();

  await widget.getByTitle("Начать помидор").click();
  await expect(widget.locator(".pomo-label")).toHaveText("Фокус");

  await widget.getByTitle("Остановить").click();
  await expect(widget.getByTitle("Начать помидор")).toBeVisible();
});

test("дашборд: карточка «Помодоро» показывает статистику и стрики", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], settings: { onboarding_complete: true },
    pomodoroStats: { today: 3, week: 12, task_streak: 4, pomodoro_streak: 2 },
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Дашборд" }).click();

  const card = page.locator(".card.panel", { hasText: "Помодоро" });
  await expect(card).toBeVisible();
  await expect(card).toContainText("3");
  await expect(card).toContainText("12");
  await expect(card).toContainText("4 дн.");
  await expect(card).toContainText("2 дн.");
});

test("дашборд: годовой календарь — квадрат сегодняшнего дня, hover показывает задачи", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "сделанное дело");
  await page.locator(".task-check").click();

  await page.getByRole("button", { name: "Дашборд" }).click();
  const p = (n: number) => String(n).padStart(2, "0");
  const now = new Date();
  const today = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;

  const cell = page.locator(`.cal-cell[data-date="${today}"]`);
  await expect(cell).toHaveAttribute("data-count", "1");

  await cell.hover();
  await expect(page.locator(".cal-tip")).toContainText("выполнено: 1");
  await expect(page.locator(".cal-tip")).toContainText("сделанное дело");
});

// Без подписей по краям сетка года не читалась: квадрат есть, а какой это день
// и месяц — непонятно. Подписи дней стоят в отдельной колонке-жёлобе слева, и
// проверять надо именно то, что жёлоб не сдвинул сетку: месяцы должны остаться
// над своими квадратами, а сами квадраты — не заезжать в колонку с подписями.
// Сетка дашборда двухколоночная, а часть панелей условная (цели, время по
// проектам, приложения, сайты). При нечётном числе узких панелей последняя
// оставалась одна в левой колонке, и справа зияла дыра до следующей широкой
// панели — ровно это было видно на скриншоте под «Резюме».
test("дашборд: последняя узкая панель не оставляет дыру справа", async ({ page }) => {
  const now = new Date().toISOString();
  // Только выполненные задачи: условные панели (проекты, приложения) не
  // отрисуются, и «Резюме» окажется последней узкой — тот самый случай.
  const tasks = Array.from({ length: 6 }, (_, i) => ({
    id: `t${i}`, title: `t${i}`, status: "Done", priority: "Medium",
    category: "Work", tags: [], hidden: true, subtasks: [],
    created_at: now, updated_at: now, completed_at: now,
  }));
  await seedDb(page, { notes: [], tasks, settings: { onboarding_complete: true } });
  await withMock(page);
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await page.getByRole("button", { name: "Дашборд" }).click();
  await page.locator(".grid .panel").first().waitFor();

  const holes = await page.evaluate(() => {
    const grid = document.querySelector(".grid") as HTMLElement;
    const gw = grid.getBoundingClientRect().width;
    const panels = [...grid.children] as HTMLElement[];
    const out: string[] = [];
    for (let i = 0; i < panels.length; i++) {
      const p = panels[i];
      const next = panels[i + 1];
      const r = p.getBoundingClientRect();
      const half = r.width < gw * 0.9;
      // Узкая панель, за которой идёт широкая (или ничего), обязана растянуться:
      // делить строку ей не с кем.
      const alone = half && (!next || next.classList.contains("wide"));
      if (alone) out.push(p.querySelector(".section-title")?.textContent?.trim() ?? "?");
    }
    return out;
  });

  expect(holes, "узкая панель осталась одна в строке — справа пустое место").toEqual([]);
});

test("дашборд: у года есть подписи дней и месяцев, сетка не съезжает", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "сделанное дело");
  await page.locator(".task-check").click();
  await page.getByRole("button", { name: "Дашборд" }).click();
  await page.locator(".cal-grid").waitFor();

  // Названы через день: на мелкой клетке семь подписей подряд сливаются.
  // Пустые всё равно отрисованы, иначе подписи разъехались бы по чужим строкам.
  await expect(page.locator(".cal-wd")).toHaveCount(7);
  await expect(page.locator(".cal-wd").filter({ hasText: /\S/ })).toHaveCount(4);
  await expect(page.locator(".cal-wd").first()).toHaveText("Пн");

  const geom = await page.evaluate(() => {
    const wds = [...document.querySelectorAll(".cal-wd")] as HTMLElement[];
    const cells = [...document.querySelectorAll(".cal-cell")] as HTMLElement[];
    const months = [...document.querySelectorAll(".cal-month")] as HTMLElement[];
    const gutterRight = wds[0].getBoundingClientRect().right;
    return {
      // Автопоток колонками должен начать квадраты со ВТОРОЙ колонки: подписи
      // занимают все семь строк первой. Иначе первая неделя легла бы в жёлоб.
      squaresInGutter: cells.filter(c => c.getBoundingClientRect().left < gutterRight - 0.5).length,
      // Подпись месяца ровно над первым числом СВОЕГО месяца. Сравнивать надо
      // именно с этим квадратом: колонки одинаковой ширины, поэтому сдвинутая
      // на одну подпись всё равно попадёт над каким-нибудь квадратом, и
      // проверка «есть хоть один» прошла бы на сломанной вёрстке.
      monthsAligned: months.every(m => {
        const first = cells.find(c => {
          const d = c.getAttribute("data-date");
          return !!d && d.endsWith("-01") &&
            new Date(d + "T00:00:00").toLocaleDateString("ru-RU", { month: "short" }).startsWith(
              (m.textContent ?? "").slice(0, 3));
        });
        return !!first &&
          Math.abs(first.getBoundingClientRect().x - m.getBoundingClientRect().x) < 1.5;
      }),
      monthCount: months.length,
    };
  });

  expect(geom.squaresInGutter, "квадраты заехали в колонку подписей").toBe(0);
  expect(geom.monthsAligned, "подпись месяца не над своим квадратом").toBe(true);
  expect(geom.monthCount).toBeGreaterThan(0);
});

// Наведение на месяц обводит все его квадраты. Главное здесь — что подсветка
// совпадает с месяцем день в день: сетка сложена колонками-неделями, а неделя
// заходит в соседний месяц с обоих концов, поэтому любая подсветка «по колонкам»
// захватила бы чужие дни. Проверяем именно границы, а не факт подсветки.
test("дашборд: наведение на месяц обводит ровно его дни", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "сделанное дело");
  await page.locator(".task-check").click();
  await page.getByRole("button", { name: "Дашборд" }).click();
  await page.locator(".cal-grid").waitFor();

  // Второй ярлык, а не первый: первый месяц года обрезан началом диапазона,
  // и на нём проверка границ ничего бы не доказала.
  const label = page.locator(".cal-month").nth(1);
  await label.hover();

  const lit = page.locator(".cal-cell.on");
  await expect(lit.first()).toBeVisible();

  const bounds = await page.evaluate(() => {
    const on = [...document.querySelectorAll(".cal-cell.on")] as HTMLElement[];
    const dates = on.map(c => c.dataset.date!).sort();
    const months = new Set(on.map(c => c.dataset.month));
    const first = dates[0], last = dates[dates.length - 1];
    const daysInMonth = new Date(
      Number(first.slice(0, 4)), Number(first.slice(5, 7)), 0).getDate();
    return { months: [...months], first, last, count: on.length, daysInMonth };
  });

  // Ровно один месяц — ни одного дня из соседних.
  expect(bounds.months.length, "подсветка залезла в соседний месяц").toBe(1);
  // От первого до последнего числа, и столько квадратов, сколько дней в месяце.
  expect(bounds.first.endsWith("-01"), `подсветка начинается не с 1-го: ${bounds.first}`).toBe(true);
  expect(bounds.last.slice(8)).toBe(String(bounds.daysInMonth).padStart(2, "0"));
  expect(bounds.count).toBe(bounds.daysInMonth);

  // Наводить можно и на сам квадрат: ярлык узкий, а квадратов у месяца три десятка.
  await page.mouse.move(0, 0);
  await expect(page.locator(".cal-cell.on")).toHaveCount(0);
  const someCell = page.locator(".cal-cell[data-date]").nth(200);
  const key = await someCell.getAttribute("data-month");
  await someCell.hover();
  const viaCell = await page.evaluate(() =>
    [...new Set([...document.querySelectorAll(".cal-cell.on")].map(c => (c as HTMLElement).dataset.month))]);
  expect(viaCell).toEqual([key]);
});

test("дашборд: клик по дню открывает попап, клик по задаче ведёт в раздел Задач", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await createTask(page, "сделанное дело");
  await page.locator(".task-check").click();

  await page.getByRole("button", { name: "Дашборд" }).click();
  const p = (n: number) => String(n).padStart(2, "0");
  const now = new Date();
  const today = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;

  const cell = page.locator(`.cal-cell[data-date="${today}"]`);
  await cell.click();

  const popup = page.locator(".cal-popup");
  await expect(popup).toBeVisible();
  await expect(popup).toContainText("выполнено: 1");
  await popup.getByRole("button", { name: "сделанное дело" }).click();

  await expect(page.locator(".cal-popup")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Задачи" })).toBeVisible();

  // Регресс: задача завершена (история) — должна открыться read-only
  // TaskHistoryDetail, а не редактируемая TaskModal (без select "Повтор"/
  // "Редактировать задачу" в заголовке — тех полей у выполненной задачи
  // уже нет смысла трогать).
  await expect(page.locator(".dialog-title", { hasText: "сделанное дело" })).toBeVisible();
  await expect(page.getByLabel("Повтор")).toHaveCount(0);
  await expect(page.getByText("Редактировать задачу")).toHaveCount(0);
});

test("сортировка: drag строки меняет порядок, порядок переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  for (const title of ["первая", "вторая", "третья"]) {
    await page.locator(".composer-input").click();
    await page.keyboard.type(title);
    await page.keyboard.press("Control+Enter");
    await expect(page.locator(".task-main", { hasText: title })).toBeVisible();
  }
  const titles = page.locator(".task-list .task-title");
  await expect(titles).toHaveText(["первая", "вторая", "третья"]);

  // тащим «первую» на «третью» → уходит в конец
  await page.locator(".task-row", { hasText: "первая" })
    .dragTo(page.locator(".task-row", { hasText: "третья" }));
  await expect(titles).toHaveText(["вторая", "третья", "первая"]);

  // порядок сохранён в «БД» и переживает перезагрузку
  await page.reload();
  await expect(page.locator(".task-list .task-title")).toHaveText(["вторая", "третья", "первая"]);
});

test("категории: создание в настройках, назначение задаче, удаление с переназначением", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // создать категорию «Спорт» в настройках (вкладка «Категории»)
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  const catSection = page.locator("section").filter({ hasText: "Категории задач" });
  await page.getByPlaceholder("Новая категория").fill("Спорт");
  await catSection.getByRole("button", { name: "Добавить" }).click();
  // 5 посевных + «Спорт» + строка добавления
  await expect(catSection.locator(".rule-row")).toHaveCount(7);
  const sportInput = catSection.locator(".rule-row input:not(.cat-color)").nth(5);
  await expect(sportInput).toHaveValue("Спорт");

  // создать задачу с этой категорией
  await page.getByRole("button", { name: "Задачи" }).click();
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  await page.getByPlaceholder("Название задачи").fill("пробежка");
  await pickOption(page, page, "Категория", "Спорт");
  await page.getByRole("button", { name: "Создать" }).click();
  // .task-meta, а не просто .chip-cat: с v0.9.99 такой же чип есть в ряду
  // фильтра по категориям, и без привязки к строке локатор находит оба.
  await expect(page.locator(".task-meta .chip-cat", { hasText: "Спорт" })).toBeVisible();

  // удалить категорию — задача переезжает в «Другое»
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  const sportRow = catSection.locator(".rule-row").nth(5);
  await expect(sportRow.locator("input:not(.cat-color)")).toHaveValue("Спорт");
  await sportRow.getByTitle("Удалить (задачи перейдут в «Другое»)").click();
  await expect(catSection.locator(".rule-row")).toHaveCount(6);

  await page.getByRole("button", { name: "Задачи" }).click();
  await expect(page.locator(".task-meta .chip-cat", { hasText: "Другое" })).toBeVisible();
});

test("лимиты категорий приложений: поле сохраняется и переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  // windowTracking включает секцию правил приложений (gated на неё же, что и
  // сама категоризация) — патчим mock-db напрямую, не через seedDb, чтобы не
  // конфликтовать с init-скриптом на reload (seedDb иначе стирает то, что
  // сохранил мок в localStorage за время теста).
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.windowTracking = "kitty";
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  const limitsLabel = page.getByText("Лимиты времени на категории (мин/день)");
  await expect(limitsLabel).toBeVisible();

  const otherRow = page.locator(".limit-row", { hasText: "Другое" });
  await otherRow.locator("input[type=number]").fill("45");
  await waitSettingsSaved(page);

  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  await expect(page.locator(".limit-row", { hasText: "Другое" }).locator("input[type=number]")).toHaveValue("45");
});

test("версии заметок: панель показывает историю, восстановление меняет текст", async ({ page }) => {
  const noteId = "n1";
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: noteId, title: "заметка с историей", content: "новый текст",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    noteRevisions: [{
      id: "rev1", note_id: noteId, content: "старый текст",
      created_at: new Date(Date.now() - 20 * 60000).toISOString(),
    }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "заметка с историей" }).click();
  await expect(noteEditor(page)).toContainText("новый текст");

  await noteAction(page, "Версии заметки");
  await expect(page.getByText("Версии заметки")).toBeVisible();
  await expect(page.locator(".revision-item")).toHaveCount(1);

  await page.locator(".revision-item").click();
  await expect(page.locator(".revision-preview pre")).toContainText("старый текст");

  page.once("dialog", (d) => d.accept());
  await page.getByRole("button", { name: "Восстановить" }).click();
  await expect(page.locator(".revisions-dialog")).toHaveCount(0);
  await expect(noteEditor(page)).toContainText("старый текст");
});

test("картинки в заметках: ![](имя) рендерится img-виджетом", async ({ page }) => {
  const noteId = "n1";
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: noteId, title: "заметка с картинкой", content: "текст ![](photo.png) конец",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    images: [{ filename: "photo.png", dataUrl: "data:image/png;base64,AAAA" }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "заметка с картинкой" }).click();

  const img = page.locator(".cm-note-image");
  await expect(img).toBeVisible();
  await expect(img).toHaveAttribute("src", "data:image/png;base64,AAAA");
});

test("картинки в заметках: ссылка скрыта по умолчанию, клик по картинке показывает/прячет её", async ({ page }) => {
  const noteId = "n1";
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: noteId, title: "заметка с картинкой", content: "![](photo.png)",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    images: [{ filename: "photo.png", dataUrl: "data:image/png;base64,AAAA" }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "заметка с картинкой" }).click();

  const editor = noteEditor(page);
  const img = page.locator(".cm-note-image");
  await expect(img).toBeVisible();
  await expect(editor).not.toContainText("![](photo.png)");

  await img.click();
  await expect(editor).toContainText("![](photo.png)");
  await expect(img).toBeVisible();

  await img.click();
  await expect(editor).not.toContainText("![](photo.png)");
  await expect(img).toBeVisible();
});

test("граф заметок: связанные заметки дают узлы и ребро, изолированная — узел без связей", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [
      {
        id: "n1", title: "Идея A", content: "см. [[Заметка Б]]",
        tags: [], linked_task_id: null, project_id: null,
        created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
      },
      {
        id: "n2", title: "Заметка Б", content: "ссылается назад на [[Идея A]]",
        tags: [], linked_task_id: null, project_id: null,
        created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
      },
      {
        id: "n3", title: "Одинокая заметка", content: "без ссылок",
        tags: [], linked_task_id: null, project_id: null,
        created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
      },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Граф" }).click();

  await expect(page.getByText("3 заметок · 1 связей")).toBeVisible();
  const nodes = page.locator(".node");
  await expect(nodes).toHaveCount(3);
  await expect(page.locator(".node.isolated")).toHaveCount(1);
  await expect(page.locator(".edge")).toHaveCount(1);

  // Двойной клик по узлу открывает заметку в разделе «Заметки»
  await page.locator(".node", { hasText: "Идея A" }).dblclick();
  await expect(page.locator(".note-item.active", { hasText: "Идея A" })).toBeVisible();
});


// v0.9.76: до этой версии заметка удалялась НАВСЕГДА, хотя у задач Корзина была.
// Несимметрия, которая рано или поздно стоит потерянного текста.
test("корзина заметок: удалённая восстанавливается с текстом", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [
      { id: "n1", title: "Черновик", content: "важный текст", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: now, updated_at: now },
      { id: "n2", title: "Вторая", content: "другое", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: now, updated_at: now },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();

  await page.locator(".note-item", { hasText: "Черновик" }).click();
  await noteAction(page, "Удалить заметку");
  await expect(page.locator(".note-row", { hasText: "Черновик" })).toHaveCount(0);

  // В Корзине — с сохранённым содержимым
  await page.getByRole("button", { name: "Корзина" }).click();
  await expect(page.locator(".note-row.trashed", { hasText: "Черновик" })).toBeVisible();

  await page.locator(".note-row.trashed", { hasText: "Черновик" }).getByTitle("Восстановить").click();
  await expect(page.locator(".note-row.trashed")).toHaveCount(0);

  // Кнопка «Заметки» есть и в навигации, и в переключателе списка — берём вторую.
  await page.locator(".seg").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "Черновик" }).click();
  await expect(page.locator(".cm-content")).toContainText("важный текст");
});

// notes_fts синхронизируется триггерами на INSERT/UPDATE/DELETE, а мягкое удаление —
// это UPDATE. Значит строка остаётся в индексе, и отсеивать её обязан сам запрос.
//
// NB: при жёстком удалении этот тест тоже зелёный — он не отличает старое
// поведение от нового, а страхует от регрессии в фильтре. Настоящую проверку
// «фильтр в SQL действительно есть» держит Rust-тест
// notes::tests::trashed_note_is_not_found_by_search, который на живой FTS падает,
// если убрать `AND n.deleted_at IS NULL`.
test("корзина заметок: удалённая не находится поиском", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [{ id: "n1", title: "уникальноеслово", content: "текст", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: now, updated_at: now }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "уникальноеслово" }).click();
  await noteAction(page, "Удалить заметку");

  await page.keyboard.press("ControlOrMeta+KeyK");
  await page.locator(".overlay input").first().fill("уникальноеслово");
  await expect(page.locator(".overlay").getByText("уникальноеслово")).toHaveCount(0);
});

// v0.9.73: пустое состояние графа. Экран без заметок не должен показывать
// пустой холст — там объяснение, что граф появится вместе со связями.
test("граф заметок: без заметок вместо пустого холста объяснение", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true } });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Граф" }).click();

  await expect(page.getByText("Пока нет заметок")).toBeVisible();
  await expect(page.locator(".node")).toHaveCount(0);
  // Холста нет вовсе (не пустой svg): .canvas живёт в {:else} ветке.
  // Проверять `svg` глобально нельзя — в навигации 14 иконок-svg.
  await expect(page.locator(".canvas")).toHaveCount(0);
});

// v0.9.73: наведение приглушает всё, что не связано с узлом под курсором —
// на большом графе это единственный способ увидеть окрестность одной заметки.
test("граф заметок: наведение приглушает несвязанные узлы", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [],
    notes: [
      { id: "n1", title: "Первая", content: "см. [[Вторая]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
      { id: "n2", title: "Вторая", content: "без ссылок", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
      { id: "n3", title: "Третья", content: "сама по себе", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Граф" }).click();
  await waitForGraphSettled(page);

  await expect(page.locator(".node.dim")).toHaveCount(0);

  // Наводим на «Первая»: связанная «Вторая» остаётся яркой, «Третья» гаснет.
  await page.locator(".node", { hasText: "Первая" }).hover();
  await expect(page.locator(".node.dim")).toHaveCount(1);
  await expect(page.locator(".node.dim", { hasText: "Третья" })).toBeVisible();
  await expect(page.locator(".node.dim", { hasText: "Вторая" })).toHaveCount(0);
});

// v0.9.73: симуляция обязана останавливаться сама. Пока она крутится, узел
// нельзя надёжно кликнуть — он уезжает из-под курсора; плюс это впустую
// потраченный CPU на экране, который просто открыли.
test("граф заметок: симуляция останавливается сама", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [],
    notes: [
      { id: "n1", title: "Первая", content: "см. [[Вторая]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
      { id: "n2", title: "Вторая", content: "см. [[Первая]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Граф" }).click();

  // Бросит исключение, если координаты продолжают меняться.
  await waitForGraphSettled(page);

  // И остаётся стоять: без остановки цикла узлы продолжали бы ползти.
  const at = () => page.locator(".node").first().getAttribute("transform");
  const settled = await at();
  await page.waitForTimeout(400);
  expect(await at()).toBe(settled);
});

// Перетаскивание узла: позиция указателя применяется раз в кадр, а рект
// контейнера кэшируется на время драга (иначе getBoundingClientRect на
// каждый pointermove — синхронный layout flush; WebKitGTK шлёт их ~170/сек).
test("граф заметок: узел следует за курсором при перетаскивании", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [],
    notes: [
      { id: "n1", title: "Тянем", content: "см. [[Вторая]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
      { id: "n2", title: "Вторая", content: "без ссылок", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Граф" }).click();
  await waitForGraphSettled(page); // иначе узел уедет сам, пока мы его меряем

  const node = page.locator(".node").first();
  const before = (await node.boundingBox())!;
  await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
  await page.mouse.down();
  for (let i = 1; i <= 20; i++) {
    await page.mouse.move(before.x + before.width / 2 + i * 6, before.y + before.height / 2 + i * 4);
  }
  await page.mouse.up();
  await page.waitForTimeout(400);

  const after = (await node.boundingBox())!;
  expect(Math.hypot(after.x - before.x, after.y - before.y)).toBeGreaterThan(30);
});

// Регрессия: узел обязан двигаться ПОКА кнопка зажата, а не прыгать на месте
// после отпускания. Тест выше этого не ловил — он мерил позицию только после
// mouse.up(), то есть ровно в тот момент, когда сломанный код всё-таки
// перерисовывал DOM (draggingId менял $state и запускал честный ререндер).
//
// Причина была в реактивности: тик делал `positions = new Map(pos)`, но это
// поверхностная копия — объекты {x,y} внутри те же, а физика мутирует их на
// месте. Svelte считал их непрочитанными и разметку не обновлял. Во время
// драга менялись только координаты внутри объекта, поэтому DOM стоял.
test("граф заметок: узел двигается во время перетаскивания, а не только после отпускания", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [],
    notes: [
      { id: "n1", title: "Тянем", content: "см. [[Вторая]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
      { id: "n2", title: "Вторая", content: "без ссылок", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Граф" }).click();
  await waitForGraphSettled(page); // иначе узел уедет сам, пока мы его меряем

  const node = page.locator(".node").first();
  const before = (await node.boundingBox())!;
  const cx = before.x + before.width / 2, cy = before.y + before.height / 2;

  await page.mouse.move(cx, cy);
  await page.mouse.down();
  for (let i = 1; i <= 20; i++) await page.mouse.move(cx + i * 6, cy + i * 4);
  // Кадру нужно успеть отрисоваться, но кнопку НЕ отпускаем.
  await page.waitForTimeout(200);

  const during = (await node.boundingBox())!;
  await page.mouse.up();

  // Со сломанной реактивностью узел здесь стоял на месте (сдвиг ~0).
  expect(Math.hypot(during.x - before.x, during.y - before.y)).toBeGreaterThan(80);
});

// Регрессия: два RAF-цикла разом. $effect перезапускался и затирал rafId, не
// отменив прежний кадр — старый цикл оставался сиротой. Замеры на живой машине
// ловили 137–145 тиков/с при экране 72Гц, то есть двойную физику и двойную
// отрисовку впустую. Считаем реальные вызовы tick через отрисованные кадры.
test("граф заметок: крутится ровно один цикл симуляции", async ({ page }) => {
  const now = new Date().toISOString();
  await seedDb(page, {
    tasks: [],
    notes: [
      { id: "n1", title: "Один", content: "см. [[Два]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
      { id: "n2", title: "Два", content: "см. [[Один]]", tags: [], linked_task_id: null, project_id: null, created_at: now, updated_at: now },
    ],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  // Считаем, сколько раз за кадр меняется transform узла: один цикл -> один
  // раз, два параллельных -> два. Ставим счётчик до входа в раздел.
  await page.evaluate(() => {
    (window as any).__writes = 0;
    const orig = SVGElement.prototype.setAttribute;
    SVGElement.prototype.setAttribute = function (name: string, value: string) {
      if (name === "transform") (window as any).__writes++;
      return orig.call(this, name, value);
    };
  });

  await page.getByRole("button", { name: "Граф" }).click();
  await page.waitForTimeout(100);
  const frames = await page.evaluate(async () => {
    (window as any).__writes = 0;
    let f = 0;
    await new Promise<void>(res => {
      const step = () => { if (++f >= 30) return res(); requestAnimationFrame(step); };
      requestAnimationFrame(step);
    });
    return { writes: (window as any).__writes, frames: f };
  });

  // 2 узла: один цикл даёт <=2 записи на кадр, два цикла — вчетверо.
  const perFrame = frames.writes / frames.frames;
  expect(perFrame).toBeLessThanOrEqual(2.6);
});

test("закрепление заметок: пин поднимает заметку наверх списка, переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  // Сеем заметки напрямую в localStorage мока (не через seedDb-init-script):
  // seedDb регистрирует свой initScript, который заново стирает localStorage
  // на каждый page.reload() — теряя то, что сохранил мок за время теста
  // (тот же грабли, что и в тесте «лимиты категорий приложений» выше).
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.notes = [
      { id: "n1", title: "Первая заметка", content: "текст 1", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: new Date(Date.now() - 2000).toISOString(), updated_at: new Date(Date.now() - 2000).toISOString() },
      { id: "n2", title: "Вторая заметка", content: "текст 2", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: new Date(Date.now() - 1000).toISOString(), updated_at: new Date(Date.now() - 1000).toISOString() },
      { id: "n3", title: "Третья заметка", content: "текст 3", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
    ];
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();

  const rows = page.locator(".note-row");
  await expect(rows).toHaveCount(3);
  const unpinnedFirstTitle = await rows.nth(0).locator(".note-title").innerText();

  // Закрепляем самую старую — "Первая заметка" — она должна подняться наверх
  const firstRow = page.locator(".note-row", { hasText: "Первая заметка" });
  await firstRow.locator(".pin-btn").click({ force: true });
  await expect(rows.nth(0)).toContainText("Первая заметка");
  await expect(page.locator(".pin-btn.pinned")).toHaveCount(1);

  // Переживает перезагрузку
  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await expect(page.locator(".note-row").nth(0)).toContainText("Первая заметка");
  await expect(page.locator(".pin-btn.pinned")).toHaveCount(1);

  // Открепление возвращает к порядку без пина (тот же, что был до закрепления)
  await page.locator(".note-row", { hasText: "Первая заметка" }).locator(".pin-btn").click({ force: true });
  await expect(page.locator(".note-row").nth(0)).toContainText(unpinnedFirstTitle);
  await expect(page.locator(".pin-btn.pinned")).toHaveCount(0);
});

test("zen-режим редактора: кнопка и хоткей раскрывают на весь экран, скрывают панель и мету, Esc закрывает", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: "n1", title: "заметка для дзена", content: "текст заметки",
      tags: ["важное"], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "заметка для дзена" }).click();

  await expect(page.locator(".list-pane")).toBeVisible();
  await expect(page.locator(".editor-meta")).toBeVisible();

  await page.getByTitle("Zen-режим (Ctrl+Shift+Z)").click();
  await expect(page.locator(".editor-pane.zen")).toBeVisible();
  await expect(page.locator(".editor-meta")).toHaveCount(0);
  await expect(noteEditor(page)).toBeVisible();
  // Список заметок технически остаётся в DOM (то же .notes-дерево), но
  // редактор — fixed-оверлей поверх него на весь экран.
  await expect(page.locator(".editor-pane.zen")).toHaveCSS("position", "fixed");

  await page.keyboard.press("Escape");
  await expect(page.locator(".editor-pane.zen")).toHaveCount(0);
  await expect(page.locator(".editor-meta")).toBeVisible();

  // Хоткей включает и выключает так же, как кнопка
  await page.keyboard.press("Control+Shift+KeyZ");
  await expect(page.locator(".editor-pane.zen")).toBeVisible();
  await page.keyboard.press("Control+Shift+KeyZ");
  await expect(page.locator(".editor-pane.zen")).toHaveCount(0);
});

test("панель форматирования: кнопки оборачивают выделение markdown-маркерами, Ctrl+B работает как хоткей", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();

  const editor = noteEditor(page);
  await editor.click();
  await page.keyboard.insertText("hello");
  await page.keyboard.press("ControlOrMeta+a");
  await page.getByTitle("Жирный (Ctrl+B)").click();
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n");

  await page.getByTitle("Чек-лист").click();
  await page.keyboard.insertText("пункт списка");
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n");

  await page.getByTitle(/Вики-ссылка/).click();
  await page.keyboard.insertText("другая заметка");
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n");

  // Курсор сейчас на новой (последней) строке — предыдущие строки больше не
  // "сырые", декорации на них должны быть отрендерены.
  await expect(page.locator(".cm-strong", { hasText: "hello" })).toBeVisible();
  await expect(page.locator(".cm-task-checkbox")).toHaveCount(1);
  await expect(page.locator(".cm-wikilink", { hasText: "другая заметка" })).toBeVisible();

  // Ctrl+B как хоткей: выделяем "hello" (уже **hello**) заново и снимаем жирный.
  await page.keyboard.press("ControlOrMeta+Home");
  await page.keyboard.press("Shift+End");
  await page.keyboard.press("ControlOrMeta+b");
  await page.keyboard.press("ControlOrMeta+Home"); // курсор больше не на этой строке — декорация должна вернуться

  // Сохранённый markdown — источник истины (декорации могут визуально
  // прятать сырые маркеры, textContent() DOM тут ненадёжен).
  await page.waitForTimeout(1000);
  const saved = await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db") || "{}");
    return db.notes?.[0]?.content ?? "";
  });
  expect(saved).toContain("hello"); // жирный снят Ctrl+B, markers should be gone
  expect(saved).not.toContain("**hello**");
  expect(saved).toContain("- [ ] пункт списка");
  expect(saved).toContain("[[другая заметка]]");
});

// v0.9.72: свой keymap подключён ДО defaultKeymap, и порядок обязателен — иначе
// комбинация уходит в стандартный обработчик, а свой не вызывается вовсе. Ctrl+B
// это уже проверяет выше; Ctrl+Shift+K не был закреплён ничем, а после выноса
// тел команд в lib/editor/ он идёт через делегат.
test("Ctrl+Shift+K доходит до своего keymap, а не до defaultKeymap", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await fillNoteEditor(page, "цель");

  await page.keyboard.press("ControlOrMeta+Home");
  await page.keyboard.press("Shift+End");
  await page.keyboard.press("ControlOrMeta+Shift+KeyK");

  await page.waitForTimeout(1000);
  const saved = await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db") || "{}");
    return db.notes?.[0]?.content ?? "";
  });
  expect(saved).toContain("[[цель]]");
});

test("таблицы в заметках: рендерится <table>, ячейка редактируется кликом, +строка/+столбец, курсор внутри блока показывает сырой markdown", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: "n1", title: "заметка с таблицей",
      content: "текст до\n\n| Имя | Возраст |\n| --- | ---: |\n| Аня | 30 |\n| Боб | 7 |\n\nтекст после",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "заметка с таблицей" }).click();

  await expect(page.locator(".cm-table")).toBeVisible();
  await expect(page.locator(".cm-table th")).toHaveCount(2);
  await expect(page.locator(".cm-table td")).toHaveCount(4);

  // Редактирование ячейки кликом: выделяем весь текст внутри именно этой
  // ячейки (не всего документа — это была реальная регрессия при разработке,
  // contenteditable="false" на обёртке виджета не создаёт отдельный edit-host
  // для Selection API в Chromium) и заменяем.
  await page.locator(".cm-table td", { hasText: "Аня" }).click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("Оля");
  await page.keyboard.press("Tab"); // коммитит правку и переходит в соседнюю ячейку

  await page.getByRole("button", { name: "+ строка" }).click();
  await page.getByRole("button", { name: "+ столбец" }).click();

  await page.waitForTimeout(1000); // автосейв
  const saved = await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db") || "{}");
    return db.notes?.[0]?.content ?? "";
  });
  expect(saved).toContain("Оля");
  expect(saved).not.toContain("| Аня ");
  expect(saved).toMatch(/\|\s*\|\s*\|\s*\|\s*\n/); // добавленная пустая строка (3 столбца после +столбец)
  expect(saved).toContain("Колонка 3"); // авто-название нового столбца
  expect(saved).toContain("текст до");
  expect(saved).toContain("текст после");

  // Печатание таблицы с нуля: пока курсор на строке заголовка/разделителя,
  // виджет не подменяет текст (иначе редактировать вслепую) — рендерится
  // только когда курсор покидает диапазон строк таблицы.
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  const editor = noteEditor(page);
  await editor.click();
  await page.keyboard.insertText("| A | B |");
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n");
  await page.keyboard.insertText("| --- | --- |");
  await expect(page.locator(".cm-table")).toHaveCount(0);
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n");
  await expect(page.locator(".cm-table")).toBeVisible();

  // Кнопка "Таблица" в панели форматирования вставляет стартовую 2x2-таблицу.
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await editor.click();
  await page.keyboard.insertText("текст перед вставкой");
  await page.getByTitle("Таблица").click();
  await expect(page.locator(".cm-table")).toBeVisible();
  await expect(page.locator(".cm-table th")).toHaveCount(2);
  await expect(page.locator(".cm-table td")).toHaveCount(4);
});

test("ИИ по выделению в редакторе: меню действий, предпросмотр результата, подтверждение заменяет выделение", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true, ai_provider: "local" } });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();

  const editor = noteEditor(page);
  await editor.click();
  await page.keyboard.insertText("исходный текст для правки");
  await page.keyboard.press("ControlOrMeta+a");

  await expect(page.getByRole("button", { name: "Сократить" })).toBeVisible();
  await page.getByRole("button", { name: "Сократить" }).click();

  const preview = page.locator(".selection-preview");
  await expect(preview).toBeVisible();
  await expect(preview).toHaveText("[shorten] исходный текст для правки");

  await page.getByTitle("Заменить выделение").click();
  await expect(page.locator(".selection-menu")).toHaveCount(0);

  await page.waitForTimeout(1000);
  const saved = await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db") || "{}");
    return db.notes?.[0]?.content ?? "";
  });
  expect(saved).toBe("[shorten] исходный текст для правки");
});

test("ИИ: резюме заметки — кнопка открывает окно с результатом, клик по тексту копирует и закрывает", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: "n1", title: "Длинная заметка", content: "много текста для резюме",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true, ai_provider: "local" },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "Длинная заметка" }).click();

  await noteAction(page, "ИИ: резюме заметки");

  const summaryText = page.locator(".summary-text");
  await expect(summaryText).toBeVisible();
  await expect(summaryText).toContainText("Пункт резюме");

  await summaryText.click();
  await expect(page.locator(".summary-dialog")).toHaveCount(0);

  // Проверяется и само копирование, а не только закрытие окна: до v0.9.80 вызов
  // writeText проваливался в немокнутую команду и молча возвращал undefined,
  // поэтому половина названия теста («клик копирует») ничем не подтверждалась.
  const copied = await page.evaluate(
    () => JSON.parse(localStorage.getItem("__mock_db")!).clipboardWritten,
  );
  expect(copied).toContain("Пункт резюме");
});

test("ИИ: извлечение задач из заметки — список с галочками, текст правится до создания, создаётся только отмеченное", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: "n1", title: "Заметка с делами", content: "нужно спланировать поездку",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true, ai_provider: "local" },
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "Заметка с делами" }).click();

  await noteAction(page, "ИИ: извлечь задачи из заметки");

  // v0.9.44: список строк с галочками вместо ряда чипов
  const rows = page.locator(".extracted-row");
  await expect(rows).toHaveCount(2);

  // value задан свойством, а не атрибутом — сверяем через toHaveValue
  const titles = page.locator(".extracted-title");
  await expect(titles.nth(0)).toHaveValue("Купить билеты");
  await expect(titles.nth(1)).toHaveValue("Забронировать отель");
  const tickets = rows.nth(0);
  const hotel = rows.nth(1);

  // отмечены по умолчанию — обычный сценарий «принять почти всё»
  await expect(page.getByRole("button", { name: "Создать: 2" })).toBeVisible();

  // снятая галочка убирает пункт из счётчика, но строка остаётся видимой
  await hotel.locator('input[type="checkbox"]').uncheck();
  await expect(page.getByRole("button", { name: "Создать: 1" })).toBeVisible();
  await expect(hotel).toBeVisible();

  // текст правится до создания — формулировки модели черновые
  await tickets.locator(".extracted-title").fill("Купить билеты на поезд");

  await page.getByRole("button", { name: "Создать: 1" }).click();

  // создана только отмеченная, с правкой; неотмеченная осталась в панели
  await expect(rows).toHaveCount(1);
  await expect(page.locator(".extracted-title")).toHaveValue("Забронировать отель");

  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await expect(page.locator(".task-row", { hasText: "Купить билеты на поезд" })).toBeVisible();
  await expect(page.locator(".task-row", { hasText: "Забронировать отель" })).toHaveCount(0);
});

test("экспорт заметки в HTML: кнопка сохраняет самодостаточный HTML-файл с заголовком и контентом", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: "n1", title: "Заметка для экспорта", content: "# Заголовок\n\nТекст с **жирным** и [[wiki-ссылкой]].",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true },
    mockDialogPath: "/mock/export/note.html",
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.locator(".note-item", { hasText: "Заметка для экспорта" }).click();
  await noteAction(page, "Экспорт в HTML");

  await expect.poll(async () => {
    const raw = await page.evaluate(() => localStorage.getItem("__mock_db"));
    return raw ? JSON.parse(raw).exportedHtml?.path : null;
  }).toBe("/mock/export/note.html");
  const db = JSON.parse(await page.evaluate(() => localStorage.getItem("__mock_db")!));
  const html = db.exportedHtml.html as string;
  expect(html).toContain("<title>Заметка для экспорта</title>");
  expect(html).toContain("<h1>Заголовок</h1>");
  expect(html).toContain("<strong>жирным</strong>");
  expect(html).toContain("wiki-ссылкой");
});

test("экспорт/импорт заметок в .md: roundtrip через папку", async ({ page }) => {
  await seedDb(page, {
    tasks: [],
    notes: [{
      id: "n1", title: "моя заметка", content: "содержимое заметки",
      tags: [], linked_task_id: null, project_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true },
    mockDialogPath: "/mock/notes-export",
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();
  await page.getByRole("button", { name: "Экспорт заметок (.md)" }).click();
  await expect(page.getByText("Экспортировано заметок: 1")).toBeVisible();

  // Импорт из той же (мок-)папки: совпадение по названию не мёржится —
  // создаётся отдельная заметка (задокументированное поведение), поэтому
  // после импорта в списке две заметки с одинаковым названием.
  await page.getByRole("button", { name: "Импорт заметок из папки" }).click();
  await expect(page.getByText("Импортировано заметок: 1")).toBeVisible();

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await expect(page.locator(".note-item", { hasText: "моя заметка" })).toHaveCount(2);
});

test("шаблоны чеклистов: сохранить подзадачи как шаблон, применить к другой задаче", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // v0.8.3: шаблоны перенесены из панели строки в модалку задачи.
  await page.getByRole("button", { name: "+ Новая", exact: true }).click();
  const modal = page.locator(".modal");
  await modal.getByPlaceholder("Название задачи").fill("поездка");
  await modal.locator(".checklist-editor").click();
  await page.keyboard.insertText("[ ] паспорт\n[ ] билеты");

  // Сохраняем как шаблон прямо из модалки создания
  await modal.getByRole("button", { name: "Сохранить как шаблон" }).click();
  await modal.getByPlaceholder("Название шаблона").fill("Поездка");
  await modal.getByRole("button", { name: "Сохранить", exact: true }).click();

  await modal.getByRole("button", { name: "Создать" }).click();
  await createTask(page, "другая задача");

  // Применяем шаблон к «другой задаче»
  const otherRow = page.locator(".task-row", { hasText: "другая задача" });
  await otherRow.locator(".task-main").dblclick();
  const otherModal = page.locator(".modal");
  await otherModal.getByRole("button", { name: "Из шаблона…" }).click();
  // Имя шаблона именно в списке шаблонов: с v0.9.56 в модалке есть ещё и
  // селект блокеров, куда попадает одноимённая задача.
  await expect(otherModal.locator(".template-panel").getByText("Поездка")).toBeVisible();
  await otherModal.getByRole("button", { name: "Применить" }).click();
  await expect(otherModal.locator(".checklist-editor")).toHaveText("паспортбилеты");
  await otherModal.getByRole("button", { name: "Сохранить", exact: true }).click();

  // Чип N/M совпадает: 2 подзадачи, 0 выполнено
  await expect(otherRow.locator(".chip-sub")).toHaveText("▾ 0/2");
});

test("тёмная тема применяется и переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  // Тема переключается сегментами (.seg), а не радиокнопками: родные радио в
  // WebKitGTK рисуются системной темой GTK мимо токенов.
  await page.getByRole("radio", { name: "Тёмная" }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);

  await waitSettingsSaved(page);
  await page.reload();
  await expect(page.locator("html")).toHaveClass(/dark/);
});

test("пресет цветов: задаёт основной и дополнительный акцент, переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await choosePreset(page, "Nord");

  const accent = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim());
  const secondary = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent-secondary").trim());
  expect(accent).toBe("#88c0d0");
  expect(secondary).toBe("#b48ead");

  await waitSettingsSaved(page);
  await page.reload();

  const accentAfter = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim());
  const secondaryAfter = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent-secondary").trim());
  expect(accentAfter).toBe("#88c0d0");
  expect(secondaryAfter).toBe("#b48ead");
});

test("настройки: сохраняются сами, кнопки «Сохранить» нет", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Кнопки формы нет — есть расписка о том, что запись идёт сама.
  await expect(page.locator(".settings > .btn-primary")).toHaveCount(0);
  await expect(page.locator(".autosave-note")).toContainText("сохраняются сами");

  // Дискретный выбор пишется без ожидания следующего нажатия.
  await choosePreset(page, "Nord");
  await waitSettingsSaved(page);

  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  expect(await page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue("--accent").trim()
  )).toBe("#88c0d0");
});

test("настройки: правка в поле сохраняется без клика по кнопке", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Ввод с клавиатуры — путь с дебаунсом, в отличие от выбора пресета.
  await page.locator(".settings-tab", { hasText: "Уведомления" }).click();
  await page.locator("input[type=time]").first().fill("07:30");
  await waitSettingsSaved(page);

  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Уведомления" }).click();
  await expect(page.locator("input[type=time]").first()).toHaveValue("07:30");
});

test("пресеты: выпадающий список открывается, показывает выбранное и закрывается", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  const trigger = page.locator(".preset-trigger");
  const list = page.locator(".preset-list");

  // Пока ничего не выбрано — цвета свои, а не пресетные.
  await expect(trigger).toContainText("Свои цвета");
  await expect(list).toHaveCount(0);

  await trigger.click();
  await expect(list).toBeVisible();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  // Все 17 пресетов в списке, а не только влезшие в строку.
  await expect(page.locator(".preset-option")).toHaveCount(17);

  await page.locator(".preset-option", { hasText: "Nord" }).click();
  // Выбор закрывает список и подставляется в кнопку.
  await expect(list).toHaveCount(0);
  await expect(trigger).toContainText("Nord");
  await expect(trigger).toHaveAttribute("aria-expanded", "false");

  // Ручная правка акцента снимает имя пресета: подпись отражает состояние, а не
  // запомненный клик.
  const advanced = page.locator("label.check", { hasText: "Акцент" }).first();
  await advanced.locator("input.color-input").evaluate((el: HTMLInputElement) => {
    el.value = "#123456";
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await expect(trigger).toContainText("Свои цвета");
});

test("пресеты: наведение показывает тему целиком и отпускает её при уходе", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  const varOf = (name: string) =>
    page.evaluate((n) => document.documentElement.style.getPropertyValue(n).trim(), name);

  await page.locator(".preset-trigger").first().click();
  const ember = page.locator(".preset-option", { hasText: "Ember" });

  await ember.hover();
  // Превью показывает не свотч, а настоящую тему: подложку и всё выведенное.
  expect(await varOf("--bg-primary")).toBe("#0d1117");
  expect(await varOf("--bg-card")).not.toBe("");
  expect(await varOf("--accent")).toBe("#ff6b35");

  // Уход мышью со списка возвращает прежнее состояние.
  await page.locator(".section-title", { hasText: "Внешний вид" }).hover();
  expect(await varOf("--bg-primary")).toBe("");
  expect(await varOf("--accent")).toBe("");
});

test("пресеты: увиденное в превью не сохраняется без клика", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  await page.locator(".preset-trigger").first().click();
  await page.locator(".preset-option", { hasText: "Gruvbox" }).hover();
  await page.keyboard.press("Escape");

  // Escape — один из трёх путей выхода; все обязаны снять превью.
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-primary").trim()
  )).toBe("");

  await waitSettingsSaved(page);
  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  // В базу просмотренный пресет не попал.
  await expect(page.locator(".preset-trigger").first()).toContainText("Свои цвета");
});

test("пресеты: список закрывается по Escape и ходит стрелками", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  const trigger = page.locator(".preset-trigger");
  await trigger.focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".preset-list")).toBeVisible();
  // Фокус ушёл на первый пункт, а не остался на кнопке.
  await expect(page.locator(".preset-option").first()).toBeFocused();

  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".preset-option").nth(1)).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(page.locator(".preset-list")).toHaveCount(0);
});

test("пресет Ember: приносит свой фон, поверхности выводятся из него", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Остальные пресеты меняют только акценты; этот несёт собственную подложку,
  // а карточки, границы и подписи обязаны пойти за ней.
  await choosePreset(page, "Ember");

  const vars = await page.evaluate(() => {
    const s = getComputedStyle(document.documentElement);
    const get = (n: string) => s.getPropertyValue(n).trim();
    return {
      accent: get("--accent"),
      secondary: get("--accent-secondary"),
      bg: get("--bg-primary"),
      card: get("--bg-card"),
      border: get("--border"),
      textSecondary: get("--text-secondary"),
    };
  });

  expect(vars.accent).toBe("#ff6b35");
  expect(vars.secondary).toBe("#ff9f1c");
  expect(vars.bg).toBe("#0d1117");
  // Выведенные: не пустые, не равны подложке и светлее её — фон почти чёрный.
  for (const name of ["card", "border"] as const) {
    expect(vars[name], name).toMatch(/^#[0-9a-f]{6}$/);
    expect(vars[name], name).not.toBe(vars.bg);
  }
  // Текст подписей на почти чёрном обязан быть светлым.
  expect(vars.textSecondary).toBe("#a0a0a0");
});

test("переключение темы снимает фон, оставшийся от пресета", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Ember приносит почти чёрную подложку. Выведенные цвета пишутся inline на
  // <html> и бьют любой класс — без сброса светлая тема осталась бы тёмной, и
  // выхода из этого состояния в интерфейсе не было бы.
  await choosePreset(page, "Ember");
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-primary").trim()
  )).toBe("#0d1117");

  await page.getByRole("radio", { name: "Светлая" }).check();
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-primary").trim()
  )).toBe("");
  // Акценты — выбор про приложение, а не про режим: они остаются.
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--accent").trim()
  )).toBe("#ff6b35");
});

test("кнопка «Вернуть фон темы» появляется только при заданном фоне и снимает его", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  const restore = page.getByRole("button", { name: "Вернуть фон темы" });
  await expect(restore).toHaveCount(0);

  await choosePreset(page, "Ember");
  await expect(restore).toBeVisible();

  await restore.click();
  await expect(restore).toHaveCount(0);
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-card").trim()
  )).toBe("");
});

test("светлый пресет уводит приложение из тёмной темы, тёмный — обратно", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Производная модель покрывает только то, что считается из фона. Класс `dark`
  // продолжает править остальным — цветами категорий, чипом тега, --danger, —
  // поэтому светлый пресет под `.dark` красил бы их для чужой подложки.
  await choosePreset(page, "Solarized");
  await expect(page.locator("html")).not.toHaveClass(/dark/);
  await expect(page.getByRole("radio", { name: "Светлая" })).toBeChecked();

  await choosePreset(page, "Dracula");
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.getByRole("radio", { name: "Тёмная" })).toBeChecked();
});

test("текст на заливке акцента следует за акцентом, а не прибит к белому", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Gruvbox: жёлтый акцент, на котором белый текст даёт 1.70.
  await choosePreset(page, "Gruvbox");
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--on-accent").trim()
  )).toBe("#141414");

  // Solarized: тёмно-синий, здесь выигрывает белый.
  await choosePreset(page, "Solarized");
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--on-accent").trim()
  )).toBe("#ffffff");
});

test("пресет со своим фоном сбрасывает ручные переопределения поверхностей", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Сначала руками задаём «Второй план» — он переопределяет выведенное.
  const advanced = page.locator("details.advanced-colors");
  await advanced.locator("summary").click();
  await advanced.locator("label.check", { hasText: "Второй план" })
    .locator("input.color-input")
    .evaluate((el: HTMLInputElement) => {
      el.value = "#00ff00";
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
  expect(await page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue("--bg-secondary").trim()
  )).toBe("#00ff00");

  // Пресет с собственной подложкой обязан его снять: иначе ярко-зелёная панель
  // осталась бы поверх новой темы.
  await choosePreset(page, "Ember");
  expect(await page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue("--bg-secondary").trim()
  )).not.toBe("#00ff00");
});

test("настройки: ИИ-провайдер — выпадающий список переключает поля, сохранение работает", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.getByRole("tab", { name: "ИИ", exact: true }).click();
  // Свой Select вместо родного: выбираем по видимой подписи, а не по value.
  const pickProvider = (label: string) => pickSetting(page, "Провайдер", label);

  await expect(page.getByPlaceholder("sk-...")).toHaveCount(0);
  await expect(page.getByPlaceholder("sk-ant-...")).toHaveCount(0);

  await pickProvider("OpenAI");
  await expect(page.getByPlaceholder("sk-...")).toBeVisible();
  await expect(page.getByPlaceholder("sk-ant-...")).toHaveCount(0);
  await page.getByPlaceholder("sk-...").fill("sk-test-openai");

  await pickProvider("Anthropic");
  await expect(page.getByPlaceholder("sk-ant-...")).toBeVisible();
  await expect(page.getByPlaceholder("sk-...")).toHaveCount(0);
  await page.getByPlaceholder("sk-ant-...").fill("sk-ant-test");

  await waitSettingsSaved(page);
  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.getByRole("tab", { name: "ИИ", exact: true }).click();

  // Кнопка показывает выбранное подписью, а не value — toHaveValue тут неприменим.
  await expect(page.locator("label", { hasText: "Провайдер" })
    .locator("button[aria-haspopup=listbox]")).toContainText("Anthropic");
  await expect(page.getByPlaceholder("sk-ant-...")).toHaveValue("sk-ant-test");
});

test("настройки: список локальных моделей — карточки с описанием и требованиями, выбор переключает URL для скачивания", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true, ai_provider: "local" } });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.getByRole("tab", { name: "ИИ", exact: true }).click();

  // v0.9.64: на вкладке ИИ теперь два загрузчика (чат-модель и распознавание
  // речи), поэтому проверки привязаны к секции чат-модели. Ослабить их до
  // .first() значило бы перестать различать пикеры вообще.
  const llmPicker = page.locator(".model-picker").filter({ hasText: "Qwen2.5" });

  // Рекомендованная модель выбрана по умолчанию, у неё бейдж "рекомендуется"
  const recommendedOption = llmPicker.locator("label", { hasText: "Qwen2.5 1.5B Instruct" });
  await expect(recommendedOption).toBeVisible();
  await expect(recommendedOption.locator("input[type=radio]")).toBeChecked();
  await expect(llmPicker.getByText("рекомендуется")).toBeVisible();

  // У каждой модели видно размер, требования по ОЗУ и описание
  await expect(llmPicker.getByText(/ГБ · от \d+ ГБ ОЗУ/).first()).toBeVisible();
  await expect(llmPicker.getByText("Самая быстрая и лёгкая")).toBeVisible();

  // Выбор другой модели переключает, какая скачается
  const phiOption = llmPicker.locator("label", { hasText: "Phi-3.5 Mini Instruct" });
  await phiOption.click();
  await expect(phiOption.locator("input[type=radio]")).toBeChecked();
  await expect(recommendedOption.locator("input[type=radio]")).not.toBeChecked();

  // Свой URL — переключает на custom, поле редактируемое
  await page.getByPlaceholder("https://.../model.gguf").fill("https://example.com/custom.gguf");
  await expect(llmPicker.locator("label", { hasText: "Свой URL" }).locator("input[type=radio]")).toBeChecked();

  // Выбор в одном пикере не трогает другой: у радиогрупп имя зависит от kind.
  // Проверяется на варианте «Свой URL» — он единственный вне {#each}, и именно
  // там имя группы легко забыть развести (так и случилось при написании).
  const whisperPicker = page.locator(".model-picker").filter({ hasText: "Whisper" });
  await whisperPicker.locator("label", { hasText: "Свой URL" }).locator("input[type=radio]").check();
  await expect(whisperPicker.locator("label", { hasText: "Свой URL" }).locator("input[type=radio]")).toBeChecked();
  // Выбор «Свой URL» у распознавания не должен сбрасывать его же у чат-модели.
  await expect(llmPicker.locator("label", { hasText: "Свой URL" }).locator("input[type=radio]")).toBeChecked();
});

test("настройки: хоткеи — переназначение применяется, дефолтная комбинация перестаёт работать", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  const hotkeysSection = page.locator("section", { hasText: "Хоткеи" });
  const settingsRow = hotkeysSection.locator(".keybind-row", { hasText: "Перейти: Настройки" });

  // Комбинация берётся из реестра, а не вписана литералом: тест про механику
  // переназначения, а не про то, какая цифра досталась Настройкам. С литералом
  // он падал при любой перенумерации разделов (v0.10.11 — Ctrl+5 -> Ctrl+6).
  const settingsDefault = formatCombo(
    KEYBIND_ACTIONS.find(a => a.id === "view_settings")!.defaultCombo,
  );
  const settingsDefaultKey = settingsDefault.replace("Ctrl+", "");

  await expect(settingsRow.locator(".keybind-combo")).toHaveText(settingsDefault);

  await settingsRow.locator(".keybind-combo").click();
  await page.keyboard.press("Control+9");
  await expect(settingsRow.locator(".keybind-combo")).toHaveText("Ctrl+9");

  await waitSettingsSaved(page);

  // Без перезагрузки: старая комбинация больше не переключает на
  // Настройки, новая Ctrl+9 работает сразу (App.svelte подхватывает
  // сохранённые хоткеи по событию keybinds-saved, не только при onMount).
  await page.getByRole("button", { name: "Задачи" }).click();
  await page.keyboard.press(`Control+${settingsDefaultKey}`);
  await expect(page.getByRole("heading", { name: "Настройки" })).toHaveCount(0);

  await page.keyboard.press("Control+9");
  await expect(page.getByRole("heading", { name: "Настройки" })).toBeVisible();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();

  // Сброс к дефолту возвращает исходную комбинацию — тоже без reload
  const settingsRowAfter = page.locator("section", { hasText: "Хоткеи" }).locator(".keybind-row", { hasText: "Перейти: Настройки" });
  await settingsRowAfter.getByTitle("Сбросить к дефолту").click();
  await expect(settingsRowAfter.locator(".keybind-combo")).toHaveText(settingsDefault);
  await waitSettingsSaved(page);

  await page.getByRole("button", { name: "Задачи" }).click();
  await page.keyboard.press("Control+9");
  await expect(page.getByRole("heading", { name: "Настройки" })).toHaveCount(0);
  await page.keyboard.press(`Control+${settingsDefaultKey}`);
  await expect(page.getByRole("heading", { name: "Настройки" })).toBeVisible();
});

test("настройки: вкладки показывают только свои секции", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();

  // По умолчанию — «Общее»: видны «Внешний вид» и «Режим работы», остальные скрыты
  await expect(page.locator(".settings-tab.active")).toHaveText("Общее");
  await expect(page.getByText("Внешний вид")).toBeVisible();
  await expect(page.getByText("Режим работы")).toBeVisible();
  await expect(page.getByText("ИИ-провайдер")).toHaveCount(1); // в DOM
  await expect(page.getByText("ИИ-провайдер")).not.toBeVisible();

  await page.getByRole("tab", { name: "ИИ", exact: true }).click();
  await expect(page.getByText("ИИ-провайдер")).toBeVisible();
  await expect(page.getByText("Внешний вид")).not.toBeVisible();

  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  await expect(page.getByText("ИИ-провайдер")).not.toBeVisible();
  // .section-title, а не текст по всей секции: слова из разных разделов
  // приложения встречаются и в тексте Справки (v0.9.29).
  await expect(page.locator("section .section-title", { hasText: "Хоткеи" })).toBeVisible();
});

test("настройки: текст и границы спрятаны за раскрывашкой, остальные цвета видны сразу", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Три основных поля видны без единого клика: остальные поверхности выводятся
  // из фона, а не задаются отдельно.
  for (const label of ["Акцент", "Доп. акцент", "Фон"]) {
    await expect(page.locator("label.check", { hasText: label }).first()).toBeVisible();
  }

  // Переопределения выведенного — в DOM, но свёрнуты: сломать ими читаемость
  // проще всего.
  const advanced = page.locator("details.advanced-colors");
  for (const label of ["Второй план", "Карточки", "Фон наведения", "Границы", "Основной", "Подписи"]) {
    await expect(advanced.locator("label.check", { hasText: label })).not.toBeVisible();
  }

  await advanced.locator("summary").click();
  for (const label of ["Второй план", "Карточки", "Фон наведения", "Границы", "Основной", "Подписи"]) {
    await expect(advanced.locator("label.check", { hasText: label })).toBeVisible();
  }
  // «Сбросить к дефолту» уехал внутрь вместе с ними.
  await expect(advanced.getByRole("button", { name: "Сбросить к дефолту" })).toBeVisible();
  // Заголовки групп: семь полей подряд читались бы как стена.
  await expect(advanced.getByText("Поверхности")).toBeVisible();
});

test("настройки: цвет карточек переопределяется точечно и снимается крестиком", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  await page.locator("label.check", { hasText: "Фон" }).first()
    .locator("input.color-input")
    .evaluate((el: HTMLInputElement) => {
      el.value = "#7a1520";
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });

  const advanced = page.locator("details.advanced-colors");
  await advanced.locator("summary").click();
  const cards = advanced.locator("label.check", { hasText: "Карточки" });

  // Пока не тронуто — значение выведено из фона, крестика нет.
  await expect(cards.locator("input.color-input")).toHaveClass(/is-default/);
  await expect(cards.locator(".unset-btn")).toHaveCount(0);

  await cards.locator("input.color-input").evaluate((el: HTMLInputElement) => {
    el.value = "#00ff00";
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-card").trim()
  )).toBe("#00ff00");
  // Соседний слой не тронут — он всё ещё следует фону.
  expect(await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-secondary").trim()
  )).not.toBe("#00ff00");

  // Крестик возвращает выведенное значение, а не пустоту.
  await cards.locator(".unset-btn").click();
  await expect(cards.locator(".unset-btn")).toHaveCount(0);
  const back = await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--bg-card").trim()
  );
  expect(back).not.toBe("");
  expect(back).not.toBe("#00ff00");
});

test("настройки: выбор фона перекрашивает карточки и подписи, а не только полотно", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  await page.locator("label.check", { hasText: "Фон" }).first()
    .locator("input.color-input")
    .evaluate((el: HTMLInputElement) => {
      el.value = "#7a1520";
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });

  // Ровно тот дефект со скриншота: --bg-card не был настраиваемым и оставался
  // от прежней темы, поэтому строки задач были чёрными на красном.
  const vars = await page.evaluate(() => {
    const s = document.documentElement.style;
    return {
      bg: s.getPropertyValue("--bg-primary"),
      card: s.getPropertyValue("--bg-card"),
      border: s.getPropertyValue("--border"),
      textSecondary: s.getPropertyValue("--text-secondary"),
    };
  });
  expect(vars.bg).toBe("#7a1520");
  for (const [name, value] of Object.entries(vars)) {
    expect(value, name).toMatch(/^#[0-9a-f]{6}$/);
  }
  expect(vars.card).not.toBe(vars.bg);
});

test("настройки: свотч неустановленного цвета показывает дефолт темы, а не индиго", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Мок отдаёт все color_* пустыми, значит все свотчи — дефолтные.
  const bg = page.locator("label.check", { hasText: "Фон" }).first().locator("input.color-input");
  await expect(bg).toHaveValue("#ffffff");
  await expect(bg).toHaveClass(/is-default/);

  // Выбор цвета снимает пометку дефолта.
  await bg.evaluate((el: HTMLInputElement) => {
    el.value = "#123456";
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await expect(bg).not.toHaveClass(/is-default/);
});

test("настройки: свой пресет сохраняется, применяется и удаляется", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  // Пока ничего не сохранено — блока «Мои пресеты» нет.
  await expect(page.getByText("Мои пресеты")).toHaveCount(0);

  const bg = page.locator("label.check", { hasText: "Фон" }).first().locator("input.color-input");
  const setColor = async (locator: typeof bg, value: string) => {
    await locator.evaluate((el: HTMLInputElement, v) => {
      el.value = v;
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }, value);
  };

  await setColor(bg, "#102030");
  await page.locator("input.preset-name").fill("Ночной");
  await page.getByRole("button", { name: "Сохранить текущие цвета" }).click();

  // Набор появился, поле имени очистилось.
  await expect(page.getByText("Мои пресеты")).toBeVisible();
  await expect(page.locator("input.preset-name")).toHaveValue("");

  // Меняем цвет и возвращаемся к набору — он восстанавливает сохранённое.
  await setColor(bg, "#ffcc00");
  await expect(bg).toHaveValue("#ffcc00");
  await chooseCustomPreset(page, "Ночной");
  await expect(bg).toHaveValue("#102030");

  // Удаление — крестик прямо в строке списка.
  const sel = customPresetSelect(page);
  await sel.locator(".preset-trigger").click();
  await sel.locator(".option-del").click();
  await expect(page.getByText("Мои пресеты")).toHaveCount(0);
});

test("настройки: сохранённый пресет переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();

  await page.locator("label.check", { hasText: "Акцент" }).first()
    .locator("input.color-input")
    .evaluate((el: HTMLInputElement) => {
      el.value = "#abcdef";
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
  await page.locator("input.preset-name").fill("Рабочий");
  await page.getByRole("button", { name: "Сохранить текущие цвета" }).click();
  await waitSettingsSaved(page);

  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await customPresetSelect(page).locator(".preset-trigger").click();
  await expect(page.locator(".preset-option", { hasText: "Рабочий" })).toBeVisible();
});

test("настройки: поиск скрывает несовпавшие секции и переключает вкладку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  const sections = page.locator(".settings section");
  await expect(sections).toHaveCount(12); // +Справка (v0.9.29), +Голосовой ввод (v0.9.64)

  // «бэкап» — совпадение в «Авто-бэкап» (вкладка «Данные»), поиск (v0.8.10)
  // сам переключает на неё. Справка (v0.9.29) тоже объясняет бэкапы, и это
  // намеренно: искать в ней — часть смысла раздела, поэтому она в выдаче тоже.
  await page.getByPlaceholder("Поиск по настройкам…").fill("бэкап");
  await expect(page.locator(".settings-tab.active")).toHaveText("Данные");
  await expect(page.locator(".settings section:visible .section-title")).toHaveText("Авто-бэкап");

  // Очистка поиска возвращает все секции активной («Данные») вкладки —
  // без сброса вкладки на «Общее».
  await page.getByPlaceholder("Поиск по настройкам…").fill("");
  await expect(page.locator(".settings-tab.active")).toHaveText("Данные");
  await expect(page.locator(".settings section:visible")).toHaveCount(2); // Авто-бэкап + Данные

  // v0.9.64: новая секция должна быть не только на своей вкладке, но и находима
  // поиском — она добавлена последней по индексу, а SECTION_TAB сопоставляется
  // по позиции, так что рассинхрон вкладки и секции виден именно здесь.
  await page.getByPlaceholder("Поиск по настройкам…").fill("распознавания");
  await expect(page.locator(".settings-tab.active")).toHaveText("ИИ");
  await expect(page.locator(".settings section:visible .section-title")).toHaveText("Голосовой ввод");
});

// v0.9.92. Раньше confirm показывался ДО выбора файла («Импорт заменит все
// текущие данные»), то есть подтверждали вслепую, а в диалоге выбора видно лишь
// метку времени в имени. На живой установке это дважды за час привело к одному и
// тому же: выбран старый снимок, созданная после него заметка молча откатилась.
test("импорт: перед подтверждением показано содержимое копии и что будет потеряно", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.mockDialogPath = "/tmp/backup.zip";
    // Копия, которая старше текущей базы: ровно ситуация пользователя.
    db.importPreview = {
      tasks: 39, notes: 8, current_tasks: 39, current_notes: 9,
      newest: "2026-08-06T13:43:00+00:00",
      losing_tasks: 0, losing_notes: 1,
    };
    db.confirmAnswer = false; // тест только читает текст, импорт запускать не надо
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();

  // Предупреждение о закрытии видно ещё до нажатия: сообщения ПОСЛЕ импорта быть
  // не может — app.restart() не возвращается.
  await expect(page.getByText("При импорте приложение закроется")).toBeVisible();

  // Подтверждение идёт через диалог ПЛАГИНА, а не через window.confirm:
  // браузерный confirm поверх только что закрытого файлового диалога роняет
  // процесс на Linux/WebKitGTK (найдено на живой установке). Поэтому текст
  // читается из мока, а не через page.on("dialog").
  await page.getByRole("button", { name: "Импорт (ZIP)" }).click();
  const asked = await pollConfirmText(page);

  // Обе стороны и разница: «39 задач» само по себе ни о чём не говорит.
  expect(asked).toContain("Задачи: 39 → 39");
  expect(asked).toContain("Заметки: 9 → 8");
  expect(asked).toContain("−1"); // заметок в копии на одну меньше
  // Главное: чем именно рискует пользователь.
  expect(asked).toContain("ВНИМАНИЕ");
  expect(asked).toContain("заметок: 1");
});

async function pollConfirmText(page: import("@playwright/test").Page): Promise<string> {
  await expect
    .poll(async () =>
      await page.evaluate(() => JSON.parse(localStorage.getItem("__mock_db")!).lastConfirm ?? ""),
    )
    .not.toBe("");
  return await page.evaluate(
    () => JSON.parse(localStorage.getItem("__mock_db")!).lastConfirm as string,
  );
}

// Обратная половина: когда терять нечего, лишнего предупреждения быть не должно —
// иначе оно станет фоновым шумом и его перестанут читать.
test("импорт: без записей новее копии предупреждения о потере нет", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.mockDialogPath = "/tmp/backup.zip";
    db.importPreview = {
      tasks: 5, notes: 2, current_tasks: 5, current_notes: 2,
      newest: "2026-08-06T13:43:00+00:00",
      losing_tasks: 0, losing_notes: 0,
    };
    db.confirmAnswer = false;
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();

  await page.getByRole("button", { name: "Импорт (ZIP)" }).click();
  const asked = await pollConfirmText(page);

  expect(asked).toContain("Задачи: 5 → 5");
  // Равные числа — разницы в скобках быть не должно.
  expect(asked).not.toContain("(+");
  expect(asked).not.toContain("ВНИМАНИЕ");
});

test("авто-бэкап: секция в настройках, кнопка «Сделать сейчас» вызывает команду", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();

  // Секция «Авто-бэкап» видна (.section-title — слово встречается и в
  // тексте Справки на другой вкладке, v0.9.29)
  await expect(page.locator(".section-title", { hasText: "Авто-бэкап" })).toBeVisible();
  await expect(page.getByText("Папка для бэкапов")).toBeVisible();
  await expect(page.getByText("Хранить копий")).toBeVisible();

  // «Сделать сейчас» disabled без папки
  const backupBtn = page.getByRole("button", { name: "Сделать сейчас" });
  await expect(backupBtn).toBeDisabled();

  // Устанавливаем папку в настройках, сохраняем, перезагружаем
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.settings.auto_backup_dir = "/tmp/mock-backups";
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();

  // Теперь кнопка активна и вызывает команду
  await expect(backupBtn).toBeEnabled();
  await backupBtn.click();
  await expect(page.getByText("Бэкап сохранён")).toBeVisible();
});

// v0.9.85: состояние бэкапа наконец видно. До этой версии в Settings.svelte была
// переменная lastBackup, которая рисовалась в разметке и НИГДЕ не присваивалась,
// а last_auto_backup не доходил до фронтенда вообще — пользователь не мог
// отличить «бэкапы идут» от «папка не выбрана».
test("настройки: состояние авто-бэкапа видно и различает выключен/свежий/устаревший", async ({ page }) => {
  const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString();

  await withMock(page);
  await page.goto("/");

  // Хранилище правится ПОСЛЕ первой загрузки и подхватывается перезагрузкой —
  // тот же приём, что у соседнего теста про «Сделать сейчас». Через seedDb здесь
  // нельзя: его init-скрипт должен выполниться раньше мока, а мок читает
  // __mock_db один раз при инициализации, и в обратном порядке приложение вообще
  // не поднимается.
  const showState = async (dir: string, last: string, err = "") => {
    await page.evaluate(([d, l, e]) => {
      const db = JSON.parse(localStorage.getItem("__mock_db")!);
      db.settings.auto_backup_dir = d;
      db.settings.last_auto_backup = l;
      db.settings.last_auto_backup_error = e;
      localStorage.setItem("__mock_db", JSON.stringify(db));
    }, [dir, last, err]);
    await page.reload();
    await page.getByRole("button", { name: "Настройки" }).click();
    await page.locator(".settings-tab", { hasText: "Данные" }).click();
  };

  // Ровно то состояние, в котором была живая БД пользователя.
  await showState("", "");
  await expect(page.getByText("Авто-бэкап выключен: папка не выбрана.")).toBeVisible();

  // Папка есть, копий ещё не было — это ожидание, а не ошибка.
  await showState("/tmp/mock-backups", "");
  await expect(page.getByText("Папка выбрана, первая копия появится в течение суток.")).toBeVisible();

  // Свежая копия: дата показана, предупреждения нет.
  await showState("/tmp/mock-backups", hoursAgo(2));
  await expect(page.locator(".hint", { hasText: "Последний бэкап:" })).toBeVisible();
  await expect(page.locator(".hint-warn")).toHaveCount(0);

  // Двое суток без копии — предупреждение.
  await showState("/tmp/mock-backups", hoursAgo(72));
  await expect(page.locator(".hint-warn", { hasText: "старше двух суток" })).toBeVisible();

  // v0.9.86: сбой автобэкапа. Копия при этом СВЕЖАЯ — до v0.9.86 такое состояние
  // выглядело бы полностью благополучным, а цикл был уже мёртв. Ошибка обязана
  // перебивать дату, иначе проверять её было бы нечем.
  await showState("/tmp/mock-backups", hoursAgo(2), "2026-08-06T10:00:00+00:00\tОшибка файловой системы: Permission denied");
  await expect(page.locator(".hint-warn", { hasText: "не удался" })).toBeVisible();
  // Показано именно сообщение, а не сырая строка с датой и табуляцией.
  await expect(page.locator(".hint-warn", { hasText: "Permission denied" })).toBeVisible();
  await expect(page.getByText("2026-08-06T10:00:00")).toHaveCount(0);
});

test("рутины: создание, блок в неделе, выключение", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // Создаём задачу с дедлайном на сегодня (нужна для недельного вида)
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    if (!db.routines) db.routines = [];
    db.tasks.push({
      id: "test-task-1",
      title: "Тестовая задача",
      status: "Todo",
      priority: "Medium",
      category: "Other",
      tags: [],
      description: null,
      deadline: null,
      recurrence: "None",
      hidden: false,
      project_id: null,
      scheduled_at: new Date().toISOString(),
      scheduled_mins: 60,
      sort_order: 1,
      subtasks: [],
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      completed_at: null,
    });
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });

  // Переходим в календарь, затем в недельный вид
  await page.getByRole("button", { name: "Календарь" }).click();
  await page.getByRole("button", { name: "Неделя" }).click();

  // Открываем модал рутин
  await page.getByRole("button", { name: "Рутины" }).click();
  await expect(page.getByRole("heading", { name: "Рутины" })).toBeVisible();

  // Добавляем новую рутину
  await page.getByRole("button", { name: "+ Добавить рутину" }).click();
  await page.locator(".edit-form input[placeholder='Название рутины']").fill("Планёрка");
  // Включаем Пн и Вт (первые два чекбокса)
  await page.locator(".day-chip input").nth(0).check();
  await page.locator(".day-chip input").nth(1).check();
  // Время начала 09:00
  await page.locator(".edit-form input[type='time']").fill("09:00");
  await page.locator(".edit-form input[type='number']").fill("45");
  await page.getByRole("button", { name: "Добавить" }).click();

  // Рутина видна в списке модала
  await expect(page.getByRole("dialog").getByText("Планёрка")).toBeVisible();
});

test("трекинг: пункт меню запускает, он же останавливает", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    db.tasks.push({
      id: "track-1", title: "Трекинг-тест", status: "Todo", priority: "Medium",
      category: "Other", tags: [], description: null, deadline: null,
      recurrence: "None", hidden: false, project_id: null,
      scheduled_at: null, scheduled_mins: null, sort_order: 1, subtasks: [],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
      completed_at: null,
    });
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  // Запуск — из контекстного меню строки (v0.9.98)
  await expect(page.getByText("Трекинг-тест")).toBeVisible();
  await rowMenu(page, "Трекинг-тест", "Начать трекинг");

  // Виджет трекинга появился в сайдбаре
  await expect(page.locator(".track")).toContainText("Трекинг-тест");

  // Пункт меню теперь несёт обратное действие: он показывает состояние, а не
  // только запускает. Останавливаем им же.
  await rowMenu(page, "Трекинг-тест", "Остановить трекинг");
  await expect(page.locator(".track")).toHaveCount(0);
});

test("фокус-режим: тумблер в настройках сохраняется и переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Уведомления" }).click();
  const toggle = page.getByLabel("Фокус-режим: авто-пауза уведомлений на время помодоро-работы и активных тайм-блоков");
  await expect(toggle).toBeChecked();

  await flipSwitch(page, "Фокус-режим: авто-пауза уведомлений");
  await expect(toggle).not.toBeChecked();
  await waitSettingsSaved(page);

  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Уведомления" }).click();
  await expect(page.getByLabel("Фокус-режим: авто-пауза уведомлений на время помодоро-работы и активных тайм-блоков")).not.toBeChecked();
});

test("экран «Сегодня»: показывает блок дня и дедлайны, клик по каждому ведёт в задачу", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date();
    const mk = (h: number, m: number) => {
      const d = new Date(now); d.setHours(h, m, 0, 0); return d.toISOString();
    };
    db.tasks.push(
      {
        id: "today-block-1", title: "Блок сегодня", status: "Todo", priority: "Medium",
        category: "Other", tags: [], description: null, deadline: null,
        recurrence: "None", hidden: false, project_id: null,
        scheduled_at: mk(9, 0), scheduled_mins: 60, sort_order: 1, subtasks: [],
        created_at: now.toISOString(), updated_at: now.toISOString(), completed_at: null,
      },
      {
        id: "today-due-1", title: "Дедлайн сегодня", status: "Todo", priority: "High",
        category: "Other", tags: [], description: null, deadline: mk(23, 0),
        recurrence: "None", hidden: false, project_id: null,
        scheduled_at: null, scheduled_mins: null, sort_order: 2, subtasks: [],
        created_at: now.toISOString(), updated_at: now.toISOString(), completed_at: null,
      },
    );
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  await page.getByRole("button", { name: "Сегодня", exact: true }).click();
  await expect(page.getByText("Блок сегодня")).toBeVisible();
  await expect(page.getByText("Дедлайн сегодня")).toBeVisible();

  // Клик по блоку таймлайна ведёт в раздел «Задачи» с открытой карточкой
  await page.locator(".tl-block", { hasText: "Блок сегодня" }).click();
  await expect(page.locator(".modal.dialog")).toBeVisible();
  await expect(page.locator(".modal.dialog input").first()).toHaveValue("Блок сегодня");
});

test("умные списки: встроенные фильтруют список, свой список создаётся и удаляется", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date();
    const mk = (daysFromNow: number) => {
      const d = new Date(now); d.setDate(d.getDate() + daysFromNow); return d.toISOString();
    };
    db.tasks.push(
      {
        id: "sl-overdue", title: "Просроченная задача", status: "Todo", priority: "High",
        category: "Work", tags: [], description: null, deadline: mk(-2),
        recurrence: "None", hidden: false, project_id: null,
        scheduled_at: null, scheduled_mins: null, sort_order: 1, subtasks: [],
        created_at: now.toISOString(), updated_at: now.toISOString(), completed_at: null,
      },
      {
        id: "sl-other", title: "Задача другой категории", status: "Todo", priority: "Low",
        category: "Home", tags: [], description: null, deadline: null,
        recurrence: "None", hidden: false, project_id: null,
        scheduled_at: null, scheduled_mins: null, sort_order: 2, subtasks: [],
        created_at: now.toISOString(), updated_at: now.toISOString(), completed_at: null,
      },
    );
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  // Встроенный список «Просроченные» скрывает всё остальное
  await page.locator(".smart-list-chip", { hasText: "Просроченные" }).click();
  await expect(page.getByText("Просроченная задача")).toBeVisible();
  await expect(page.getByText("Задача другой категории")).not.toBeVisible();

  // «Все» возвращает полный список
  await page.locator(".smart-list-chip", { hasText: "Все" }).click();
  await expect(page.getByText("Задача другой категории")).toBeVisible();

  // Свой список по категории «Дом»
  await page.locator(".smart-list-add").click();
  await page.getByPlaceholder("Например: Важное").fill("Только дом");
  // Диалог смарт-списка — не модалка задачи: здесь <select> остался родным.
  await page.locator(".modal.dialog select").first().selectOption({ label: "Дом" });
  await page.getByRole("button", { name: "Создать", exact: true }).click();

  await page.locator(".smart-list-chip", { hasText: "Только дом" }).click();
  await expect(page.getByText("Задача другой категории")).toBeVisible();
  await expect(page.getByText("Просроченная задача")).not.toBeVisible();

  // Удаление своего списка
  await page.locator(".smart-list-chip.custom", { hasText: "Только дом" }).locator(".smart-list-remove").click();
  await expect(page.locator(".smart-list-chip", { hasText: "Только дом" })).toHaveCount(0);
  await expect(page.getByText("Просроченная задача")).toBeVisible();
});

test("мультивыбор задач: Ctrl/Shift+клик выделяет строки, массовые действия применяются ко всем", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.tasks.push(
      { id: "ms-1", title: "Мульти 1", status: "Todo", priority: "Medium", category: "Work", tags: [], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null, scheduled_at: null, scheduled_mins: null, sort_order: 1, subtasks: [], created_at: now, updated_at: now, completed_at: null },
      { id: "ms-2", title: "Мульти 2", status: "Todo", priority: "Medium", category: "Work", tags: [], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null, scheduled_at: null, scheduled_mins: null, sort_order: 2, subtasks: [], created_at: now, updated_at: now, completed_at: null },
      { id: "ms-3", title: "Мульти 3", status: "Todo", priority: "Medium", category: "Work", tags: [], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null, scheduled_at: null, scheduled_mins: null, sort_order: 3, subtasks: [], created_at: now, updated_at: now, completed_at: null },
    );
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  // Ctrl+клик по первой, Shift+клик по третьей — диапазон выделяет все три
  await page.locator(".task-main", { hasText: "Мульти 1" }).click({ modifiers: ["Control"] });
  await page.locator(".task-main", { hasText: "Мульти 3" }).click({ modifiers: ["Shift"] });
  await expect(page.locator(".bulk-bar")).toBeVisible();
  await expect(page.locator(".bulk-count")).toHaveText("3 выбрано");
  await expect(page.locator(".task-row.selected")).toHaveCount(3);

  // Двойной клик по карточке всё ещё открывает её, когда выбора нет
  await page.locator(".bulk-bar .btn-icon").click(); // снять выбор
  await expect(page.locator(".bulk-bar")).toHaveCount(0);
  await page.locator(".task-main", { hasText: "Мульти 1" }).dblclick();
  await expect(page.locator(".modal.dialog")).toBeVisible();
  await page.locator(".modal.dialog").getByRole("button", { name: "Отмена" }).click();

  // Массовое выполнение
  await page.locator(".task-main", { hasText: "Мульти 1" }).click({ modifiers: ["Control"] });
  await page.locator(".task-main", { hasText: "Мульти 2" }).click({ modifiers: ["Control"] });
  await page.getByRole("button", { name: "Выполнить", exact: true }).click();
  await expect(page.locator(".bulk-bar")).toHaveCount(0);
  await expect(page.getByText("Мульти 1")).not.toBeVisible();
  await expect(page.getByText("Мульти 2")).not.toBeVisible();
  await expect(page.getByText("Мульти 3")).toBeVisible();
});

test("мультивыбор заметок: Ctrl/Shift+клик выделяет строки, массовое удаление применяется ко всем", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.notes.push(
      { id: "mn-1", title: "Заметка А", content: "текст", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: now, updated_at: now },
      { id: "mn-2", title: "Заметка Б", content: "текст", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: now, updated_at: now },
      { id: "mn-3", title: "Заметка В", content: "текст", tags: [], linked_task_id: null, project_id: null, pinned: false, created_at: now, updated_at: now },
    );
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки", exact: true }).click();

  // Ctrl+клик по первой, Shift+клик по третьей — диапазон выделяет все три
  await page.locator(".note-item", { hasText: "Заметка А" }).click({ modifiers: ["Control"] });
  await page.locator(".note-item", { hasText: "Заметка В" }).click({ modifiers: ["Shift"] });
  await expect(page.locator(".bulk-notes-bar")).toBeVisible();
  await expect(page.locator(".bulk-notes-count")).toHaveText("3 выбрано");
  await expect(page.locator(".note-row.selected")).toHaveCount(3);

  // Обычный клик всё ещё открывает заметку в редакторе, когда выбора нет
  await page.locator(".bulk-notes-bar .btn-icon").click(); // снять выбор
  await expect(page.locator(".bulk-notes-bar")).toHaveCount(0);
  await page.locator(".note-item", { hasText: "Заметка А" }).click();
  await expect(page.locator(".title-input")).toHaveValue("Заметка А");

  // Массовое удаление
  await page.locator(".note-item", { hasText: "Заметка Б" }).click({ modifiers: ["Control"] });
  await page.locator(".note-item", { hasText: "Заметка В" }).click({ modifiers: ["Control"] });
  await page.getByRole("button", { name: "Удалить", exact: true }).click();
  await expect(page.locator(".bulk-notes-bar")).toHaveCount(0);
  await expect(page.getByText("Заметка Б")).not.toBeVisible();
  await expect(page.getByText("Заметка В")).not.toBeVisible();
  await expect(page.getByText("Заметка А")).toBeVisible();
});

test("центр уведомлений: бейдж непрочитанных, лента показывает историю, открытие панели помечает прочитанным", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date();
    db.notificationLog = [
      { id: "n1", kind: "deadline", title: "Сдать отчёт", body: "Дедлайн через 1 ч", created_at: now.toISOString(), read_at: null },
      { id: "n2", kind: "block", title: "Созвон", body: "Начался блок (до 15:00)", created_at: new Date(now.getTime() - 3600_000).toISOString(), read_at: null },
    ];
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  await expect(page.locator(".unread-badge")).toHaveText("2");

  await page.locator(".bell-item").click();
  await expect(page.locator(".notif-panel")).toBeVisible();
  await expect(page.getByText("Сдать отчёт")).toBeVisible();
  await expect(page.getByText("Созвон")).toBeVisible();

  // Открытие панели помечает всё прочитанным — бейдж пропадает после закрытия
  await page.locator(".notif-panel .btn-icon").click();
  await expect(page.locator(".notif-panel")).toHaveCount(0);
  await expect(page.locator(".unread-badge")).toHaveCount(0);

  // Очистка ленты
  await page.locator(".bell-item").click();
  await page.getByRole("button", { name: "Очистить" }).click();
  await expect(page.getByText("Уведомлений пока не было")).toBeVisible();
});

test("естественный язык в композере: !приоритет @категория #тег и дата разбираются в превью и при создании", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  const composer = page.locator(".composer-input");
  await composer.fill("завтра 15:00 созвон !высокий @работа #важное");

  // Живой предпросмотр показывает разобранные метаданные
  const preview = page.locator(".composer-preview");
  await expect(preview.getByText("Высокий")).toBeVisible();
  await expect(preview.locator(".chip-cat", { hasText: "Работа" })).toBeVisible();
  await expect(preview.locator(".chip-tag", { hasText: "#важное" })).toBeVisible();
  // Месяц считаем от «завтра», а не пишем строкой: зашитый "Jul" ломался
  // в первый же день следующего месяца — тест падал 31.07 → 01.08, хотя код
  // был исправен. Формат совпадает с тем, чем превью рисует дату.
  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  const month = tomorrow.toLocaleDateString("en-US", { month: "short" });
  await expect(preview).toContainText(month);

  await page.getByRole("button", { name: "Создать", exact: true }).click();

  // Задача создана с чистым названием и разобранными полями (не сырым текстом)
  await expect(page.getByText("созвон", { exact: true })).toBeVisible();
  await expect(page.getByText("завтра 15:00 созвон")).not.toBeVisible();
  const row = page.locator(".task-row", { hasText: "созвон" });
  await expect(row.locator(".chip-cat")).toHaveText("Работа");
  await expect(row.locator(".chip-tag")).toHaveText("#важное");
});

test("естественный язык в композере: неизвестная категория подсвечивается как ошибка в превью", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.locator(".composer-input").fill("задача @несуществующая");
  await expect(page.locator(".composer-preview .chip-danger")).toContainText("несуществующая");
});

test("напоминание у заметки: поле сохраняется и переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.notes.push({
      id: "rn1", title: "Заметка с напоминанием", content: "текст", tags: [],
      linked_task_id: null, project_id: null, pinned: false, reminder_at: null,
      created_at: now, updated_at: now,
    });
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки", exact: true }).click();
  await page.locator(".note-item", { hasText: "Заметка с напоминанием" }).click();

  await page.locator('input[type="datetime-local"]').fill("2026-08-01T10:00");
  await page.locator('input[type="datetime-local"]').blur();
  await page.waitForTimeout(200);

  await page.reload();
  await page.locator(".nav").getByRole("button", { name: "Заметки", exact: true }).click();
  await page.locator(".note-item", { hasText: "Заметка с напоминанием" }).click();
  await expect(page.locator('input[type="datetime-local"]')).toHaveValue("2026-08-01T10:00");

  // Снятие напоминания через ✕
  await page.locator('label:has-text("Напоминание") button[title="Убрать напоминание"]').click();
  await page.waitForTimeout(200);
  await expect(page.locator('input[type="datetime-local"]')).toHaveValue("");
});

test("напоминание у заметки: клик по уведомлению открывает связанную заметку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.notes.push({
      id: "rn2", title: "Напомненная заметка", content: "текст", tags: [],
      linked_task_id: null, project_id: null, pinned: false, reminder_at: null,
      created_at: now, updated_at: now,
    });
    db.notificationLog = [
      { id: "notifX", kind: "note_reminder", title: "Напомненная заметка", body: "Напоминание о заметке", created_at: now, read_at: null, entity_type: "note", entity_id: "rn2" },
    ];
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();

  // Начинаем на Задачах — клик по уведомлению должен реально переключить раздел
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await page.locator(".bell-item").click();
  await page.locator(".notif-body-btn", { hasText: "Напомненная заметка" }).click();

  await expect(page.locator(".title-input")).toHaveValue("Напомненная заметка");
  await expect(page.locator(".notif-panel")).toHaveCount(0);
});

test("авто-очистка истории: настройка сохраняется и переживает перезагрузку", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();
  const input = page.getByLabel("Авто-очистка истории (мес., 0 — выкл)");
  await expect(input).toHaveValue("0");

  await input.fill("6");
  await waitSettingsSaved(page);

  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Данные" }).click();
  await expect(page.getByLabel("Авто-очистка истории (мес., 0 — выкл)")).toHaveValue("6");
});

test("канбан: доска встроена в Задачи через переключатель Список/Доска, drag меняет статус и запускает трекинг", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.tasks.push(
      { id: "kb1", title: "Написать отчёт", status: "Todo", priority: "High", category: "Work", tags: ["важное"], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null, scheduled_at: null, scheduled_mins: null, sort_order: 1, subtasks: [], created_at: now, updated_at: now, completed_at: null },
      { id: "kb2", title: "Созвон с клиентом", status: "InProgress", priority: "Medium", category: "Work", tags: [], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null, scheduled_at: null, scheduled_mins: null, sort_order: 2, subtasks: [], created_at: now, updated_at: now, completed_at: null },
      { id: "kb3", title: "Проверить почту", status: "Done", priority: "Low", category: "Other", tags: [], description: null, deadline: null, recurrence: "None", hidden: true, project_id: null, scheduled_at: null, scheduled_mins: null, sort_order: 3, subtasks: [], created_at: now, updated_at: now, completed_at: now },
    );
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();

  // Отдельного пункта «Канбан» в сайдбаре больше нет — доска внутри Задач
  await expect(page.locator(".sidebar", { hasText: "Канбан" })).toHaveCount(0);
  await page.locator(".seg button", { hasText: "Доска" }).click();

  // Каждая задача в своей колонке, включая уже выполненную (hidden) — она
  // не должна пропадать с доски, как пропадает из активного списка в режиме Список.
  await expect(page.locator(".column", { hasText: "Todo" }).locator(".board-card")).toHaveCount(1);
  await expect(page.locator(".column", { hasText: "В работе" }).locator(".board-card")).toHaveCount(1);
  await expect(page.locator(".column", { hasText: "Готово" }).locator(".board-card")).toHaveCount(1);

  // Drag из Todo в Готово — завершает задачу (status/hidden/completed_at)
  await page.locator(".board-card", { hasText: "Написать отчёт" }).dragTo(page.locator(".column", { hasText: "Готово" }));
  await expect(page.locator(".column", { hasText: "Готово" }).locator(".board-card")).toHaveCount(2);
  await expect(page.locator(".column", { hasText: "Todo" }).locator(".board-card")).toHaveCount(0);

  // Drag "Проверить почту" из Готово в "В работе" — запускает трекинг (иконка ▶ на карточке)
  await page.locator(".board-card", { hasText: "Проверить почту" }).dragTo(page.locator(".column", { hasText: "В работе" }));
  const movedCard = page.locator(".column", { hasText: "В работе" }).locator(".board-card", { hasText: "Проверить почту" });
  await expect(movedCard).toBeVisible();
  await expect(movedCard.locator(".tracking-dot")).toBeVisible();

  // Клик по карточке открывает редактирование
  await page.locator(".board-card", { hasText: "Созвон с клиентом" }).click();
  await expect(page.locator(".modal.dialog")).toBeVisible();
  await page.locator(".modal.dialog").getByRole("button", { name: "Отмена" }).click();

  // Список и Доска используют общий фильтр по проекту/умному списку —
  // переключение обратно в Список показывает оставшуюся активную задачу
  // ("Написать отчёт" теперь Done+hidden, ушла в Историю — ожидаемо)
  await page.locator(".seg button", { hasText: "Список" }).click();
  await expect(page.getByText("Проверить почту", { exact: true })).toBeVisible();
});

// v0.10.06: сегменты переключателя одной ширины. Каждый мерился по своей
// подписи, поэтому «Список» выходил 59.8px против «Доска» 55.6 и пара стояла
// заметно кривой — тот же перекос в Активные/История/Корзина.
test("переключатель: сегменты одной ширины", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Задачи", exact: true }).click();

  const segWidths = async (i: number) =>
    await page.locator(".page-head .seg").nth(i).evaluate((seg) =>
      [...seg.querySelectorAll("button")].map((b) => +b.getBoundingClientRect().width.toFixed(2)));

  // Список/Доска — подписи разной длины, ширины должны совпасть.
  const modes = await segWidths(0);
  expect(new Set(modes).size, `сегменты режима разной ширины: ${modes.join(", ")}`).toBe(1);

  // Активные/История/Корзина — три сегмента, тот же инвариант.
  const subs = await segWidths(1);
  expect(subs.length).toBe(3);
  expect(new Set(subs).size, `сегменты подвида разной ширины: ${subs.join(", ")}`).toBe(1);

  // Вкладки Настроек — отдельный вариант .seg--underline со своей раскладкой:
  // семь подписей разной длины, растягивать их по самой длинной незачем.
  await page.getByRole("button", { name: "Настройки", exact: true }).click();
  const tabs = await page.locator(".settings-tabs").evaluate((seg) =>
    [...seg.querySelectorAll("button")].map((b) => +b.getBoundingClientRect().width.toFixed(2)));
  expect(new Set(tabs).size, "вкладки Настроек выровняли по ширине заодно").toBeGreaterThan(1);
});

// v0.10.07: колонка доходит до низа окна, а не до вычисленной наугад высоты.
// Было max-height: calc(100vh - 220px), где 220 — догадка про всё, что выше
// доски. На окне 940 колонка кончалась на 794, ниже оставалось 146px пустой
// страницы, и эти 146 были недостижимы: карточки прокручиваются внутри колонки,
// а не страницей. Высота считается от реального положения доски.
test("доска: колонка доходит до низа окна, последняя карточка прокручивается целиком", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 940 });
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    // Заведомо больше, чем влезает: колонка обязана прокручиваться.
    for (let i = 0; i < 25; i++) {
      db.tasks.push({ id: `hh${i}`, title: `Готовая задача ${i}`, status: "Done", priority: "Low",
        category: "Other", tags: [], description: null, deadline: null, recurrence: "None",
        hidden: false, project_id: null, scheduled_at: null, scheduled_mins: null,
        sort_order: i, subtasks: [], created_at: now, updated_at: now, completed_at: null });
    }
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await page.locator(".seg button", { hasText: "Доска" }).click();

  const doneCol = page.locator(".column", { hasText: "Готово" });

  // Низ колонки у нижнего края окна: 8px — это padding-bottom самой доски.
  // Границы с двух сторон: слишком большой зазор — недостижимая пустая страница
  // (исходный баг, 146px), отрицательный — колонка вылезла за нижний край.
  const gap = await doneCol.evaluate((el) =>
    window.innerHeight - Math.round(el.getBoundingClientRect().bottom));
  expect(gap, "под колонкой осталась пустая страница").toBeLessThanOrEqual(10);
  expect(gap, "колонка вылезла за нижний край окна").toBeGreaterThanOrEqual(0);

  // Собственно просьба: доскроллив колонку, видно последнюю карточку целиком.
  const last = await doneCol.evaluate(async (col) => {
    const body = col.querySelector(".column-body") as HTMLElement;
    body.scrollTop = body.scrollHeight;
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const cards = body.querySelectorAll(".board-card");
    const rect = cards[cards.length - 1].getBoundingClientRect();
    return { bottom: rect.bottom, bodyBottom: body.getBoundingClientRect().bottom, n: cards.length };
  });
  expect(last.n, "карточки не отрисовались").toBe(25);
  expect(last.bottom, "последняя карточка обрезана").toBeLessThanOrEqual(last.bodyBottom + 1);

  // Высота не константа: на узком окне шапка переносится, доска уезжает вниз,
  // и колонка обязана пересчитаться — иначе она вылезет за нижний край.
  await page.setViewportSize({ width: 700, height: 940 });
  // Оба конца снова: без пересчёта колонка сохраняет прежнюю высоту от съехавшей
  // вниз доски и вылезает за край — зазор уходит в минус, а не растёт.
  await expect
    .poll(async () => await doneCol.evaluate((el) =>
      window.innerHeight - Math.round(el.getBoundingClientRect().bottom)),
      { message: "колонка не пересчиталась под перенос шапки" })
    .toBeGreaterThanOrEqual(0);
  expect(await doneCol.evaluate((el) =>
    window.innerHeight - Math.round(el.getBoundingClientRect().bottom)),
    "после переноса шапки под колонкой пустая страница").toBeLessThanOrEqual(10);
});

// v0.10.10: Ctrl+` открывает «Сегодня», а Ctrl+Tab внутри Задач переключает
// Список/Доску. «Сегодня» ушло с Ctrl+7: это экран «что сейчас», а не седьмой
// раздел по счёту, и цифровой ряд остался за разделами.
test("хоткеи: Ctrl+` открывает Сегодня, Ctrl+7 больше не навигация", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await expect(page.locator(".page-title")).toHaveText("Задачи");

  await page.keyboard.press("Control+Backquote");
  // По корню экрана, а не по заголовку: «Сегодня» встречается ещё и в заголовках
  // секций (getByRole ловил три элемента), а свой .page-title у этого экрана
  // не используется — там .today-header h2.
  await expect(page.locator(".today-view"), "Ctrl+` не открыл Сегодня").toBeVisible();

  // Старая комбинация освободилась и никуда не ведёт.
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await expect(page.locator(".page-title")).toHaveText("Задачи");
  await page.keyboard.press("Control+7");
  await expect(page.locator(".page-title"), "Ctrl+7 всё ещё навигация").toHaveText("Задачи");
  await expect(page.locator(".today-view"), "Ctrl+7 всё ещё открывает Сегодня").toHaveCount(0);
});

test("хоткеи: Ctrl+Tab переключает Список/Доску и работает в обе стороны", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Задачи", exact: true }).click();

  const activeMode = page.locator(".page-head .seg button.active").first();
  await expect(activeMode).toHaveText("Список");

  await page.keyboard.press("Control+Tab");
  await expect(activeMode, "Ctrl+Tab не переключил на Доску").toHaveText("Доска");

  // Обратно: переключатель, а не «перейти на доску».
  await page.keyboard.press("Control+Tab");
  await expect(activeMode, "Ctrl+Tab не вернул в Список").toHaveText("Список");

  // Из Доски тоже работает: обработчик стоит до проверок курсора списка,
  // которые в режиме Доски выходят раньше.
  await page.locator(".seg button", { hasText: "Доска" }).click();
  await expect(activeMode).toHaveText("Доска");
  await page.keyboard.press("Control+Tab");
  await expect(activeMode, "из Доски Ctrl+Tab не сработал").toHaveText("Список");

  // Внутри модалки Tab ходит по её полям — переключать вид он не должен.
  await createTask(page, "задача для модалки");
  await page.locator(".task-main", { hasText: "задача для модалки" }).dblclick();
  await expect(page.locator(".modal.dialog")).toBeVisible();
  await page.keyboard.press("Control+Tab");
  await expect(page.locator(".modal.dialog"), "модалка закрылась от Ctrl+Tab").toBeVisible();
  await expect(activeMode, "Ctrl+Tab сработал поверх модалки").toHaveText("Список");
});

// v0.10.12: логотип KRIPTAG набран вшитым Space Grotesk.
// Главная ловушка: если шрифт не подгрузился, font-family молча откатывается к
// system-ui — правило выглядит рабочим, а надпись рисуется как раньше. Поэтому
// проверяется не только статус загрузки, но и что глифы ДРУГИЕ: та же строка в
// вшитом шрифте и в системном должна мериться по-разному.
test("логотип: KRIPTAG набран вшитым гротеском, а не системным шрифтом", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  const brand = page.locator(".brand");
  await expect(brand).toHaveText("KRIPTAG");

  const font = await page.evaluate(async () => {
    await (document as { fonts: FontFaceSet }).fonts.ready;
    const el = document.querySelector(".brand") as HTMLElement;
    const measure = (family: string) => {
      const ctx = document.createElement("canvas").getContext("2d")!;
      ctx.font = `700 15px ${family}`;
      return Math.round(ctx.measureText("KRIPTAG").width * 100) / 100;
    };
    return {
      family: getComputedStyle(el).fontFamily,
      loaded: (document as { fonts: FontFaceSet }).fonts.check('700 15px "Kriptag Wordmark"'),
      wordmark: measure('"Kriptag Wordmark"'),
      system: measure("system-ui"),
    };
  });

  expect(font.family, "логотип не ссылается на вшитый шрифт").toContain("Kriptag Wordmark");
  expect(font.loaded, "шрифт логотипа не загрузился").toBe(true);
  // Собственно доказательство, что это не откат на системный шрифт. Не строгое
  // неравенство, а заметный отрыв: подменив @font-face на системную гарнитуру,
  // я получил 68 против 68.21 — формально «разные», и проверка на !== молча
  // прошла. Настоящий гротеск уже 62 против 68.21, то есть отрыв больше 4px.
  expect(
    Math.abs(font.wordmark - font.system),
    `надпись мерится почти как системным шрифтом (${font.wordmark} против ${font.system}) — похоже на откат`,
  ).toBeGreaterThan(3);
});

// v0.10.06: ширина колонок Доски тянется за разделитель и переживает перезапуск.
// Каждая колонка своей ширины: разделитель двигает ту, что слева от него, и не
// трогает остальные. Ширины лежат в карте по id статуса, поэтому удалённая или
// добавленная колонка не ломает остальные.
test("доска: разделитель меняет только свою колонку и переживает перезапуск", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await page.locator(".seg button", { hasText: "Доска" }).click();

  const widths = async () =>
    await page.locator(".column").evaluateAll((els) =>
      els.map((e) => Math.round(e.getBoundingClientRect().width)));

  // Ручка у каждой колонки, включая последнюю: она стоит справа от своей
  // колонки, а не в промежутке между парой. С ручками только в промежутках
  // крайнюю правую колонку нельзя было потянуть вовсе.
  const cols = await page.locator(".column").count();
  expect(await page.locator(".col-resize").count(), "у крайней колонки нет своей ручки").toBe(cols);

  const drag = async (nth: number, dx: number) => {
    const h = page.locator(".col-resize").nth(nth);
    const b = (await h.boundingBox())!;
    await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
    await page.mouse.down();
    await page.mouse.move(b.x + b.width / 2 + dx, b.y + b.height / 2, { steps: 8 });
    await page.mouse.up();
  };

  expect(await widths()).toEqual([260, 260, 260]);

  // Первый разделитель тянет только первую колонку.
  await drag(0, 100);
  expect(await widths(), "первый разделитель задел соседей").toEqual([360, 260, 260]);

  // Второй — только вторую, и первая остаётся там, где её оставили.
  await drag(1, -50);
  expect(await widths(), "второй разделитель задел соседей").toEqual([360, 210, 260]);

  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  expect(await widths(), "ширины не пережили перезапуск").toEqual([360, 210, 260]);

  // Прижатие к границам: тянуть можно сколько угодно, ширина упрётся.
  await drag(0, -900);
  expect((await widths())[0], "нет нижней границы").toBe(180);
  await drag(0, 1400);
  expect((await widths())[0], "нет верхней границы").toBe(520);

  // Клавиатура — единственный способ без мыши. Shift даёт крупный шаг.
  await page.locator(".col-resize").first().focus();
  await page.keyboard.press("ArrowLeft");
  expect((await widths())[0], "стрелка не меняет ширину").toBe(510);
  await page.keyboard.press("Shift+ArrowLeft");
  expect((await widths())[0], "Shift не даёт крупный шаг").toBe(470);
  // Соседей клавиатура тоже не трогает.
  expect((await widths())[1], "клавиатура задела соседнюю колонку").toBe(210);

  // Последняя колонка тянется так же, как остальные, и соседей не трогает.
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  const before = await widths();
  await drag(cols - 1, 120);
  const after = await widths();
  expect(after[cols - 1], "последняя колонка не потянулась").toBe(before[cols - 1] + 120);
  expect(after.slice(0, cols - 1), "последняя ручка задела соседей").toEqual(before.slice(0, cols - 1));

  // Мусор в хранилище не должен ломать доску.
  await page.evaluate(() => {
    const st = JSON.parse(localStorage.getItem("ui_state")!);
    st.boardColWidths = "очень широко";
    localStorage.setItem("ui_state", JSON.stringify(st));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  expect(await widths(), "битое значение не откатилось к умолчанию").toEqual([260, 260, 260]);

  // Одна битая запись не должна стоить ширины остальным.
  await page.evaluate(() => {
    const st = JSON.parse(localStorage.getItem("ui_state")!);
    st.boardColWidths = { Todo: 400, InProgress: "широко" };
    localStorage.setItem("ui_state", JSON.stringify(st));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  expect(await widths(), "битая запись утащила соседей").toEqual([400, 260, 260]);
});

// v0.10.03: Доска отстала от Списка на три версии — кромка приоритета (v0.9.97),
// контурный чип категории (v0.9.99) и контекстное меню (v0.9.98) прошли мимо
// .board-card. Тест держит все три и главную ловушку доски: карточка — это
// <button> с onclick, поэтому правая кнопка не должна заодно открыть модалку.
test("доска: приоритет кромкой, контурный чип, меню по правой кнопке без модалки", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.tasks.push({
      id: "bu1", title: "Согласовать смету", status: "Todo", priority: "Critical", category: "Work",
      tags: [], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null,
      scheduled_at: null, scheduled_mins: null, sort_order: 1, subtasks: [], created_at: now, updated_at: now, completed_at: null,
    });
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await page.locator(".seg button", { hasText: "Доска" }).click();

  const card = page.locator(".board-card", { hasText: "Согласовать смету" });
  await expect(card).toBeVisible();

  // Точки внутри заголовка больше нет — она сдвигала заголовки вправо на свою
  // ширину, из-за чего в колонке они начинались с разного отступа.
  await expect(card.locator(".prio-dot")).toHaveCount(0);

  // Кромка рисуется в ::before, и её нельзя проверять через getComputedStyle:
  // тот возвращает объявленное правило даже при content: none, то есть когда на
  // экране ничего нет. Проверено — с content:none стиль по-прежнему отдавал
  // 3px и красный. Поэтому цвет снимается со скриншота: это единственное, что
  // отличает нарисованную полоску от объявленной.
  // Проверяется тем, что элемент реально занимает место: ::before растягивает
  // карточку по высоте (top/bottom: 3px), поэтому при content: none её высота
  // не изменится, а вот сама полоска исчезнет из точки под левым краем. Берётся
  // стек элементов в этой точке — псевдоэлемент отдаёт свой хозяин, поэтому
  // сравнивается ширина зазора между текстом и краем карточки.
  const geom = await card.evaluate((el) => {
    const cs = getComputedStyle(el);
    const before = getComputedStyle(el, "::before");
    const title = el.querySelector(".board-card-title") as HTMLElement;
    return {
      padLeft: cs.paddingLeft,
      content: before.content,
      width: before.width,
      bg: before.backgroundColor,
      titleLeft: title.getBoundingClientRect().left - el.getBoundingClientRect().left,
    };
  });
  // content обязателен: без него правило существует, но ничего не рисует —
  // getComputedStyle при этом всё равно отдаёт width и цвет.
  expect(geom.content, "кромка объявлена, но ничего не рисует").toBe('""');
  expect(geom.width, "у карточки нет кромки приоритета").toBe("3px");
  expect(geom.bg, "кромка не окрашена в цвет приоритета").toBe("rgb(220, 38, 38)");
  // Место под кромку зарезервировано, иначе она легла бы на текст.
  expect(geom.padLeft, "под кромку не отведено место").toBe("13px");

  // Чип категории контурный: заливка зарезервирована за активным фильтром.
  const chip = card.locator(".chip-cat");
  await expect(chip).toHaveClass(/chip-cat--edge/);
  await expect(chip).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");

  // Правая кнопка открывает то же меню, что и в Списке...
  await card.click({ button: "right" });
  await expect(page.locator('[role="menu"]')).toBeVisible();
  await expect(page.locator('[role="menuitem"]', { hasText: "Удалить" })).toBeVisible();
  // ...и при этом НЕ срабатывает onclick карточки, открывающий модалку.
  await expect(page.locator(".modal.dialog")).toHaveCount(0);

  await page.keyboard.press("Escape");
  await expect(page.locator('[role="menu"]')).toHaveCount(0);

  // Левый клик по-прежнему открывает модалку — жест старше меню.
  await card.click();
  await expect(page.locator(".modal.dialog")).toBeVisible();
});

// v0.10.04: последнее расхождение Доски со Списком — состояния, которых на
// карточке не было видно вовсе. Заблокированная задача выглядела готовой к
// работе, а прогресс по подзадачам был виден только в Списке.
// v0.10.05: шапка Задач одинаковой ширины в обоих режимах. Ограничение 1200px
// стояло на .page, и шапка его наследовала: в Списке она обрывалась, не доходя
// до края, а на Доске (1600px) шла до конца — одна и та же панель меняла ширину
// вслед за тем, что под ней. Ограничение переехало на содержимое под шапкой,
// поэтому тест держит обе половины: шапка одинаковая, а строки по-прежнему
// упираются в 1200 и не растягиваются на широком экране.
test("шапка задач: одна ширина в обоих режимах, при этом список остаётся узким", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await withMock(page);
  await page.goto("/");
  await createTask(page, "задача для ширины");

  const widthOf = async (sel: string) =>
    await page.locator(sel).evaluate((el) => Math.round(el.getBoundingClientRect().width));

  const headList = await widthOf(".page-head");
  const listWidth = await widthOf(".task-list");

  await page.locator(".seg button", { hasText: "Доска" }).click();
  const headBoard = await widthOf(".page-head");

  expect(headList, "шапка в Списке уже, чем на Доске").toBe(headBoard);

  // Вторая половина: ограничение не исчезло, а переехало. Без этого «починкой»
  // было бы просто снять max-width, и строки растянулись бы на весь монитор.
  expect(listWidth, "список потерял ограничение ширины").toBe(1200);
  expect(headList, "шапка всё ещё обрезана по ширине списка").toBeGreaterThan(1200);

  // На широком экране строки не тянутся дальше — именно ради этого cap и нужен.
  await page.setViewportSize({ width: 1920, height: 900 });
  await page.locator(".seg button", { hasText: "Список" }).click();
  expect(await widthOf(".task-list"), "на широком экране список растянулся").toBe(1200);

  // Окно шире потолка Доски (1600). Правило .board-mode > * той же
  // специфичности, что и снятие потолка с шапки, поэтому порядок правил решает
  // исход: стоя выше, .board-mode возвращал шапке потолок 1600 и ширины снова
  // расходились — тот же дефект, просто дальше по шкале. При 1440 этого не
  // видно, окно туда не достаёт.
  await page.setViewportSize({ width: 2200, height: 900 });
  const wideList = await widthOf(".page-head");
  await page.locator(".seg button", { hasText: "Доска" }).click();
  expect(await widthOf(".page-head"), "на очень широком экране шапка Доски снова уже").toBe(wideList);

  // Всё под шапкой начинается там же, где шапка. Потолок ширины не должен
  // центрировать детей: .task-list не прямой ребёнок .page и центрирование его
  // не касалось, поэтому чипы с композером уезжали вправо, а список оставался
  // на месте — три разных левых края на одном экране.
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.locator(".seg button", { hasText: "Список" }).click();
  const leftOf = async (sel: string) =>
    await page.locator(sel).first().evaluate((el) => Math.round(el.getBoundingClientRect().left));
  const headLeft = await leftOf(".page-head");
  for (const sel of [".smart-lists", ".task-list"]) {
    expect(await leftOf(sel), `${sel} не совпадает по левому краю с шапкой`).toBe(headLeft);
  }
});

test("доска: счётчик подзадач и признак блокировки на карточке", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    const base = {
      status: "Todo", priority: "Medium", category: "Work", tags: [], description: null,
      deadline: null, recurrence: "None", hidden: false, project_id: null,
      scheduled_at: null, scheduled_mins: null, subtasks: [], created_at: now,
      updated_at: now, completed_at: null,
    };
    db.tasks.push(
      { ...base, id: "sb1", title: "Частично сделана", sort_order: 1,
        subtasks: [
          { id: "s1", task_id: "sb1", title: "раз", done: true, sort_order: 1 },
          { id: "s2", task_id: "sb1", title: "два", done: false, sort_order: 2 },
          { id: "s3", task_id: "sb1", title: "три", done: false, sort_order: 3 },
        ] },
      { ...base, id: "sb2", title: "Всё отмечено", sort_order: 2,
        subtasks: [{ id: "s4", task_id: "sb2", title: "готово", done: true, sort_order: 1 }] },
      { ...base, id: "sb3", title: "Без подзадач", sort_order: 3 },
      { ...base, id: "sb4", title: "Ждёт другую", sort_order: 4 },
      { ...base, id: "sb5", title: "Держит четвёртую", sort_order: 5 },
    );
    db.taskDeps = [{ task_id: "sb4", blocker_id: "sb5" }];
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await page.locator(".seg button", { hasText: "Доска" }).click();

  const card = (title: string) => page.locator(".board-card", { hasText: title });

  // Счётчик — только цифры, без шкалы и без кнопки: разворачивать на карточке некуда.
  await expect(card("Частично сделана").locator(".chip-subs")).toHaveText("1/3");
  await expect(card("Частично сделана").locator(".sub-track")).toHaveCount(0);

  // Всё отмечено — зелёный, как в Списке.
  const done = card("Всё отмечено").locator(".chip-subs");
  await expect(done).toHaveText("1/1");
  await expect(done).toHaveClass(/subs-done/);
  await expect(card("Частично сделана").locator(".chip-subs")).not.toHaveClass(/subs-done/);

  // Без подзадач чипа нет вовсе — пустой "0/0" был бы шумом.
  await expect(card("Без подзадач").locator(".chip-subs")).toHaveCount(0);

  // Блокировка: гаснет содержимое, но не карточка — иначе погасла бы и кромка
  // приоритета, единственный цветной признак в колонке.
  const blockedCard = card("Ждёт другую");
  await expect(blockedCard).toHaveClass(/blocked/);
  await expect(blockedCard).toHaveCSS("opacity", "1");
  await expect(blockedCard.locator(".board-card-title")).toHaveCSS("opacity", "0.55");

  // Причина блокировки — в подсказке карточки, а не отдельной строкой:
  // в узкой колонке лишняя строка гоняла бы высоту карточек.
  await expect(blockedCard).toHaveAttribute("title", /Держит четвёртую/);

  // Незаблокированная карточка держит в подсказке приоритет, а не блокера.
  await expect(card("Без подзадач")).toHaveAttribute("title", /Приоритет/);
  await expect(card("Без подзадач")).not.toHaveClass(/blocked/);
});

test("канбан: своя колонка добавляется на доске, управляется в Настройках, встроенные статусы защищены", async ({ page }) => {
  // Шире дефолта: с 4+ колонками доска ощутимо шире списка задач.
  await page.setViewportSize({ width: 1600, height: 900 });
  await withMock(page);
  await page.goto("/");
  await page.evaluate(() => {
    const db = JSON.parse(localStorage.getItem("__mock_db")!);
    const now = new Date().toISOString();
    db.tasks.push({
      id: "kc1", title: "Поправить баг", status: "Todo", priority: "Medium", category: "Work",
      tags: [], description: null, deadline: null, recurrence: "None", hidden: false, project_id: null,
      scheduled_at: null, scheduled_mins: null, sort_order: 1, subtasks: [], created_at: now, updated_at: now, completed_at: null,
    });
    localStorage.setItem("__mock_db", JSON.stringify(db));
  });
  await page.reload();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();
  await page.locator(".seg button", { hasText: "Доска" }).click();

  // "+ Колонка" прямо на доске
  await page.locator(".add-column button", { hasText: "Колонка" }).click();
  await page.locator(".add-column input").fill("На ревью");
  await page.locator(".add-column input").press("Enter");
  const newColumn = page.locator(".column", { hasText: "На ревью" });
  await expect(newColumn).toBeVisible();

  // Drag задачи в новую колонку — обычный update_task(status), без спецэффектов
  await page.locator(".board-card", { hasText: "Поправить баг" }).dragTo(newColumn);
  await expect(newColumn.locator(".board-card")).toHaveCount(1);

  // Управление статусами — в Настройках
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  const statusSection = page.locator("section").filter({ hasText: "Статусы задач" });
  // 4 встроенных (Todo/В работе/Готово/Архив) + свой добавленный — тот же
  // позиционный приём, что уже использует тест категорий (sportInput.nth(5)).
  const reviewInput = statusSection.locator(".rule-row input:not(.cat-color)").nth(4);
  await expect(reviewInput).toHaveValue("На ревью");

  // Встроенные статусы (Todo/В работе/Готово/Архив) нельзя переименовать/удалить
  const todoRow = statusSection.locator(".rule-row").first();
  await expect(todoRow.getByText("встроенный")).toBeVisible();
  await expect(todoRow.locator("input:not(.cat-color)")).toBeDisabled();
  await expect(todoRow.locator("input:not(.cat-color)")).toHaveValue("Todo");

  // Свой статус можно удалить — задачи переезжают в Todo
  // (6 полей: 4 встроенных + свой + инпут формы добавления; после удаления — 5)
  const reviewRow = statusSection.locator(".rule-row").nth(4);
  await reviewRow.getByTitle(/Удалить/).click();
  await expect(statusSection.locator(".rule-row input:not(.cat-color)")).toHaveCount(5);
});

// v0.9.33: быстрый слот (Ctrl+Shift+J) — одна закреплённая задача или заметка,
// которую хоткей открывает сразу на правку текста. В отличие от остальных
// режимов quick-task.html этот ничего не создаёт, а меняет существующее.
test("быстрый слот: открывает закреплённую задачу и сохраняет правку текста", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Дописать главу", description: "план на вечер",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  // видно, что правится именно задача, а не создаётся новая
  await expect(page.locator(".pin-badge")).toHaveText("⚡ Задача");
  await expect(page.locator(".pin-title")).toHaveValue("Дописать главу");
  await expect(page.locator(".pin-text")).toHaveValue("план на вечер");
  // вкладок «Задача/Заметка» здесь нет — это не форма создания
  await expect(page.locator(".seg")).toHaveCount(0);

  await page.locator(".pin-text").fill("план на вечер\n+ сверить цитаты");
  await page.getByRole("button", { name: "Сохранить" }).click();

  await expect(page.locator(".pin-saved")).toBeVisible();
  const desc = await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].description);
  expect(desc).toBe("план на вечер\n+ сверить цитаты");
});

// Пустой слот — штатное состояние (ещё ничего не закрепляли), а не ошибка:
// окно должно объяснить, как закрепить, а не показать пустую форму.
test("быстрый слот: пустой слот объясняет, как закрепить, вместо пустой формы", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], projects: [], quickMode: "pinned" });
  await withMock(page);
  await page.goto("/quick-task.html");

  await expect(page.locator(".pin-empty-title")).toBeVisible();
  await expect(page.locator(".pin-empty-hint")).toContainText("Закрепите задачу или заметку");
  await expect(page.locator(".error")).toHaveCount(0);
  // сохранять нечего — кнопки «Сохранить» быть не должно
  await expect(page.getByRole("button", { name: "Сохранить" })).toHaveCount(0);
});

// Задача, отправленная в Корзину, не должна открываться хоткеем на правку:
// пользователь её выбросил. Удаление у задач мягкое (deleted_at), поэтому без
// явного фильтра запись бы «жила» в слоте.
test("быстрый слот: задача из Корзины читается как пустой слот", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "выброшенная", description: "текст",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      deleted_at: new Date().toISOString(),
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  await expect(page.locator(".pin-empty-title")).toBeVisible();
  await expect(page.locator(".pin-title")).toHaveCount(0);
});

// В списке заметок рядом стоят две похожие кнопки: пин (наверх списка, v0.9.02)
// и молния (быстрый слот). Тест держит их раздельными: нажатие на одну не
// должно менять состояние другой.
test("быстрый слот: молния в заметках не путается с закреплением наверх", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [{
      id: "n1", title: "заметка для слота", content: "текст",
      tags: [], linked_task_id: null, project_id: null, pinned: false,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    settings: { onboarding_complete: true },
  });
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();

  const row = page.locator(".note-row").first();
  await row.getByTitle("В быстрый слот (Ctrl+Shift+J)").click();

  // в слот легла заметка, а закрепление наверх осталось выключенным
  const db = await page.evaluate(() => JSON.parse(localStorage.getItem("__mock_db")!));
  expect(db.pinnedKind).toBe("note");
  expect(db.pinnedId).toBe("n1");
  expect(db.notes[0].pinned).toBeFalsy();

  // повторное нажатие — открепление из слота
  await row.getByTitle("Убрать из быстрого слота").click();
  const after = await page.evaluate(() => JSON.parse(localStorage.getItem("__mock_db")!));
  expect(after.pinnedId).toBe("");
});

// v0.9.34: чек-лист в слоте. Главное отличие от TaskModal — правки уходят в
// БД сразу по клику, а не по «Сохранить»: слот открывают, чтобы отметить
// сделанное и закрыть, и галочка не должна теряться на Escape.
test("быстрый слот: чек-лист задачи виден, отметка уходит в БД сразу, без «Сохранить»", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт за квартал", description: "текст",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [
        { id: "s1", task_id: "t1", title: "собрать цифры", done: true, position: 0 },
        { id: "s2", task_id: "t1", title: "отправить", done: false, position: 1 },
      ],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  // v0.9.45: разметка скрыта, отметка — чекбокс внутри строки
  const boxes = page.locator(".checklist-editor .cm-sub-checkbox");
  await expect(boxes).toHaveCount(2);
  await expect(boxes.nth(0)).toBeChecked();
  await expect(boxes.nth(1)).not.toBeChecked();
  await expect(page.locator(".checklist-editor")).toHaveText("собрать цифрыотправить");

  // отметка — без нажатия «Сохранить»
  await boxes.nth(1).click();
  await expect(boxes.nth(1)).toBeChecked();

  // Запись отложена на паузу набора (v0.9.45), поэтому ждём её, а не читаем
  // БД сразу: expect.poll перечитывает, пока не сойдётся.
  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.done))
  ).toEqual([true, true]);
});

test("быстрый слот: подзадача добавляется строкой и удаляется удалением строки", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "старая", done: false, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  // v0.9.45: добавление и удаление — это правка текста. Enter внутри редактора
  // добавляет строку и НЕ сохраняет слот целиком (иначе окно бы закрылось).
  const editor = page.locator(".checklist-editor");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.press("Enter");
  await page.keyboard.insertText("сверить с бухгалтерией");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(2);
  await expect(page.locator(".pin-saved")).toHaveCount(0);

  // удаление строки — обычное редактирование текста, отдельного крестика нет
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("[ ] сверить с бухгалтерией");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(1);

  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title))
  ).toEqual(["сверить с бухгалтерией"]);
});

// v0.9.45: запись чек-листа отложена на паузу набора, поэтому Escape обязан
// её дописать. Это тот самый сценарий, ради которого v0.9.34 делала сохранение
// мгновенным: набрал подзадачу и сразу закрыл окно — потерять её нельзя.
test("быстрый слот: Escape сразу после правки чек-листа не теряет её", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "старая", done: false, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  await page.locator(".checklist-editor").click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText("[ ] старая\n[ ] дописать вывод");
  // Escape немедленно, не дожидаясь паузы набора. Ждать записи через
  // expect.poll здесь нельзя: в браузере окно не закрывается по-настоящему,
  // отложенный таймер доживает до конца теста и дописал бы правку сам — тест
  // прошёл бы и без flushSubs в cancel(). Поэтому проверяем сразу, пока пауза
  // ещё не истекла: запись обязана быть уже сделанной самим Escape.
  await page.keyboard.press("Escape");

  const titles = await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title));
  expect(titles).toEqual(["старая", "дописать вывод"]);
});

// Чек-лист есть только у задач — у заметки его быть не должно ни в каком виде.
test("быстрый слот: у закреплённой заметки чек-листа нет", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [{
      id: "n1", title: "Черновик", content: "текст", tags: [], pinned: false,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    quickMode: "pinned", pinnedKind: "note", pinnedId: "n1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  await expect(page.locator(".pin-badge")).toHaveText("⚡ Заметка");
  await expect(page.locator(".subs")).toHaveCount(0);
  await expect(page.locator(".checklist-editor")).toHaveCount(0);
});

// v0.9.35: глобальные хоткеи стали переназначаемыми. До этой версии вкладка
// «Хоткеи» показывала только локальные и текстом сообщала, что глобальные
// менять нельзя.
test("настройки: глобальные хоткеи переназначаются и сохраняются", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  // Не hasText: "Хоткеи" — так же называется тема в Справке (v0.9.29).
  const section = page.locator("section").filter({ has: page.locator("h3.section-title", { hasText: "Хоткеи" }) });

  // Обе группы на одной вкладке, глобальные — первыми
  await expect(section.locator(".keybind-group").first()).toContainText("Глобальные");
  const slotRow = section.locator(".keybind-row", { hasText: "Быстрый слот" });
  await expect(slotRow.locator(".keybind-combo")).toHaveText("Ctrl+Shift+J");

  await slotRow.locator(".keybind-combo").click();
  await page.keyboard.press("Control+Alt+P");
  await expect(slotRow.locator(".keybind-combo")).toHaveText("Ctrl+Alt+P");

  await waitSettingsSaved(page);

  const saved = await page.evaluate(() =>
    JSON.parse(JSON.parse(localStorage.getItem("__mock_db")!).settings.global_keybinds));
  expect(saved.quick_pinned).toBe("Ctrl+Alt+KeyP");

  // переживает перезагрузку
  await page.reload();
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  await expect(
    page.locator("section", { hasText: "Хоткеи" })
      .locator(".keybind-row", { hasText: "Быстрый слот" })
      .locator(".keybind-combo")
  ).toHaveText("Ctrl+Alt+P");
});

// Глобальный хоткей перехватывает клавиши раньше окна, поэтому совпадение с
// локальным молча убило бы локальный. Проверяем оба направления конфликта.
test("настройки: глобальный хоткей не даёт занять комбинацию другого действия", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  // Не hasText: "Хоткеи" — так же называется тема в Справке (v0.9.29).
  const section = page.locator("section").filter({ has: page.locator("h3.section-title", { hasText: "Хоткеи" }) });
  const slotRow = section.locator(".keybind-row", { hasText: "Быстрый слот" });

  // конфликт с другим ГЛОБАЛЬНЫМ действием
  await slotRow.locator(".keybind-combo").click();
  await page.keyboard.press("Control+Shift+N");
  await expect(section).toContainText("Уже занято: Быстрая задача");
  // поле осталось в режиме записи — можно сразу ввести другую комбинацию
  await expect(slotRow.locator("input.keybind-combo.recording")).toBeVisible();

  await page.keyboard.press("Escape");

  // конфликт с ЛОКАЛЬНЫМ хоткеем (Ctrl+K — командная палитра)
  await slotRow.locator(".keybind-combo").click();
  await page.keyboard.press("Control+K");
  await expect(section).toContainText("Занято хоткеем в приложении");
  // проверка комбинации асинхронная (её делает бэкенд) — дожидаемся, что поле
  // всё ещё пишет, прежде чем выходить из записи
  await expect(slotRow.locator("input.keybind-combo.recording")).toBeVisible();

  await page.keyboard.press("Escape");
  // комбинация не изменилась
  await expect(slotRow.locator(".keybind-combo")).toHaveText("Ctrl+Shift+J");
});

// Одинокая клавиша без модификатора перехватила бы букву во всей системе —
// проверку делает бэкенд (global-hotkey), а не свои правила во фронте.
test("настройки: глобальный хоткей требует модификатор", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  // Не hasText: "Хоткеи" — так же называется тема в Справке (v0.9.29).
  const section = page.locator("section").filter({ has: page.locator("h3.section-title", { hasText: "Хоткеи" }) });
  const taskRow = section.locator(".keybind-row", { hasText: "Быстрая задача" });

  await taskRow.locator(".keybind-combo").click();
  await page.keyboard.press("KeyQ");
  await expect(section).toContainText("Нужен хотя бы один модификатор");

  await page.keyboard.press("Escape");
  await expect(taskRow.locator(".keybind-combo")).toHaveText("Ctrl+Shift+N");
});

// Баг, найденный при работе над v0.9.35: пока идёт запись комбинации,
// App.svelte выполнял её как хоткей — запись Ctrl+K открывала командную
// палитру поверх поля и уводила фокус, из-за чего занятую комбинацию нельзя
// было даже ввести, чтобы увидеть сообщение о конфликте.
test("настройки: во время записи хоткея приложение не выполняет комбинацию", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Хоткеи" }).click();
  const section = page.locator("section").filter({ has: page.locator("h3.section-title", { hasText: "Хоткеи" }) });
  const dailyRow = section.locator(".keybind-row", { hasText: "Заметка дня" });

  await dailyRow.locator(".keybind-combo").click();
  await page.keyboard.press("Control+K");

  // палитра не открылась и фокус остался в поле записи
  await expect(page.locator(".search-input")).toHaveCount(0);
  await expect(dailyRow.locator("input.keybind-combo.recording")).toBeFocused();

  // Escape выходит из записи, комбинация не изменилась (конфликт с палитрой
  // показан, но ничего не сохранено).
  await page.keyboard.press("Escape");
  await expect(dailyRow.locator("button.keybind-combo")).toHaveText("Ctrl+D");
});

// v0.9.36: Задачи и Заметки переведены целиком. Тест задаёт язык явно —
// остальные ~100 тестов по-прежнему работают на прибитом к моку русском.
test("язык: экраны Задачи и Заметки переведены целиком", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [{
      id: "n1", title: "Draft", content: "text", tags: [], pinned: false,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // --- Задачи ---
  await page.getByRole("button", { name: "Tasks", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Tasks" })).toBeVisible();
  await expect(page.getByPlaceholder("Search tasks…")).toBeVisible();
  await expect(page.getByRole("button", { name: "Projects" })).toBeVisible();
  await expect(page.getByRole("button", { name: "+ New", exact: true })).toBeVisible();
  // пустое состояние и подсказки под ним — тоже переведены
  await expect(page.locator(".empty")).toContainText("No active tasks.");
  // сегменты Активные/История/Корзина
  await expect(page.locator(".seg button", { hasText: "History" })).toBeVisible();
  await expect(page.locator(".seg button", { hasText: "Trash" })).toBeVisible();
  // ни одной кириллической строки на экране не осталось
  const tasksText = await page.locator(".page").first().innerText();
  expect(tasksText).not.toMatch(/[а-яА-ЯёЁ]/);

  // --- Заметки ---
  await page.getByRole("button", { name: "Notes", exact: true }).click();
  await expect(page.getByRole("button", { name: "+ New note" })).toBeVisible();
  await expect(page.getByPlaceholder("Search...")).toBeVisible();
  await expect(page.locator(".empty")).toContainText("Select a note");
  // Заметки — своя разметка без .page, поэтому берём область контента целиком
  const notesText = await page.locator("main, .notes-view, .content").first().innerText();
  expect(notesText).not.toMatch(/[а-яА-ЯёЁ]/);
});

// v0.9.37: Календарь, Дашборд и модалка задачи. Тот же приём, что в v0.9.36 —
// ищем не конкретные строки, а любой кириллический символ: именно он ловит
// то, что собирается кодом и глазами при беглой проверке не видно.
test("язык: Календарь, Дашборд и модалка задачи переведены целиком", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  // Задача создаётся через UI, до переключения языка: так же, как в остальных
  // тестах — seedDb здесь дал бы строку без полей, которые ждёт список.
  await createTask(page, "Report");

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // --- Календарь ---
  await page.getByRole("button", { name: "Calendar", exact: true }).click();
  await expect(page.getByRole("button", { name: "Week" })).toBeVisible();
  const calText = await page.locator("main, .page, .content").first().innerText();
  expect(calText).not.toMatch(/[а-яА-ЯёЁ]/);

  // --- Дашборд ---
  await page.getByRole("button", { name: "Dashboard", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  const dashText = await page.locator("main, .page, .content").first().innerText();
  expect(dashText).not.toMatch(/[а-яА-ЯёЁ]/);

  // --- Модалка задачи ---
  await page.getByRole("button", { name: "Tasks", exact: true }).click();
  await page.locator(".task-main").first().dblclick();
  await expect(page.locator(".modal")).toBeVisible();
  // Категории и статусы намеренно исключены: это НЕ строки интерфейса, а
  // строки в БД (миграции 0015/0029), которые пользователь переименовывает и
  // дополняет своими. Переводить их значило бы затирать его собственные
  // названия, поэтому здесь проверяется только разметка модалки.
  const modalText = await page.locator(".modal").innerText();
  const withoutUserData = modalText
    .split("\n")
    .filter(line => !/^(Работа|Учёба|Дом|Здоровье|Другое|Todo|В работе|Готово|Архив)$/.test(line.trim()))
    .join("\n");
  expect(withoutUserData).not.toMatch(/[а-яА-ЯёЁ]/);
});

// v0.9.38: последние экраны фронта — «Сегодня», палитра и центр уведомлений.
// Тот же приём: ищем любой кириллический символ, а не конкретные строки.
test("язык: экран «Сегодня», палитра и уведомления переведены целиком", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // --- Сегодня ---
  await page.getByRole("button", { name: "Today", exact: true }).click();
  await expect(page.locator(".today-header h2")).toContainText("Today");
  const todayText = await page.locator("main, .page, .content").first().innerText();
  expect(todayText).not.toMatch(/[а-яА-ЯёЁ]/);

  // --- Командная палитра ---
  await page.keyboard.press("Control+k");
  await expect(page.getByPlaceholder("Search tasks and notes...")).toBeVisible();
  const paletteText = await page.locator(".overlay, .backdrop").first().innerText();
  expect(paletteText).not.toMatch(/[а-яА-ЯёЁ]/);
  await page.keyboard.press("Escape");

  // --- Центр уведомлений ---
  await page.locator(".bell-item").click();
  const panel = page.locator(".notif-panel");
  if (await panel.count()) {
    expect(await panel.innerText()).not.toMatch(/[а-яА-ЯёЁ]/);
  }
});

// v0.9.40: системный заголовок окна убран, вместо него — свои кнопки
// в правом верхнем углу.
test("кнопки окна: свои вместо системного заголовка, не перекрывают контент", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [], settings: { onboarding_complete: true } });
  await withMock(page);
  await page.goto("/");

  // Три кнопки в правом верхнем углу
  const controls = page.locator(".titlebar .win-btn");
  await expect(controls).toHaveCount(3);

  // Прижаты к правому краю окна
  const box = (await page.locator(".titlebar .controls").boundingBox())!;
  const vw = page.viewportSize()!.width;
  expect(vw - (box.x + box.width)).toBeLessThan(20);

  // Шапка не съедает клики по контенту под собой: полоса тянется во всю
  // ширину, и без pointer-events:none левая часть верхней строки стала бы
  // мёртвой зоной. Проверяем именно кликом, а не чтением стиля.
  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await expect(page.locator(".notes")).toBeVisible();
  await page.getByRole("button", { name: "Задачи", exact: true }).click();

  // Поиск в шапке Задач не оказывается под кнопками окна
  const search = (await page.getByPlaceholder("Поиск задач…").boundingBox())!;
  const btns = (await page.locator(".titlebar .controls").boundingBox())!;
  expect(search.x + search.width).toBeLessThanOrEqual(btns.x + 1);
});

// v0.9.46: Настройки и Sidebar. Эти экраны существующие языковые тесты не
// покрывали вовсе — там проверялись Задачи/Заметки/Календарь/Дашборд/Сегодня,
// а Настройки оказались почти целиком русскими (136 строк), и пользователь
// нашёл это глазами. Здесь тот же приём — ищем любой кириллический символ.
test("язык: Настройки и Sidebar переведены целиком", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // Sidebar: кнопки Поиск и Уведомления жили без t() до v0.9.46
  await expect(page.getByRole("button", { name: /Search/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /Notifications/ })).toBeVisible();

  // Каждая вкладка Настроек — своя, потому что скрытые вкладки в DOM есть, но
  // innerText их не отдаёт: проверять надо именно видимое.
  for (const tab of ["General", "AI", "Categories", "Notifications", "Data", "Hotkeys", "Help"]) {
    await page.locator(".settings-tab", { hasText: tab }).click();
    // «Русский» — единственное законное исключение: селект языка называет
    // каждый язык на нём самом (LANGS в i18n.ts), иначе выбрать родной язык
    // в чужом интерфейсе было бы нельзя.
    const shown = (await page.locator(".settings").innerText()).replace(/Русский/g, "");
    expect(shown, `вкладка ${tab}`).not.toMatch(/[а-яА-ЯёЁ]/);
  }

  // Справка разворачивается — её текст лежит в help.ts и переводится при
  // отрисовке, а не через t() в разметке.
  await page.locator(".settings-tab", { hasText: "Help" }).click();
  await page.locator(".help-topic summary").first().click();
  const help = await page.locator(".help-topic").first().innerText();
  expect(help).not.toMatch(/[а-яА-ЯёЁ]/);

  // Sidebar целиком
  const nav = await page.locator("nav, .sidebar").first().innerText();
  expect(nav).not.toMatch(/[а-яА-ЯёЁ]/);
});

// v0.9.47: категории и статусы — строки из БД, а не из разметки, поэтому ни
// один статический тест по исходникам их не видел. На английском интерфейсе
// в списке задач, канбане, модалке, пончике Дашборда и Настройках оставались
// «Работа» и «В работе». Проверяется сквозь интерфейс: модульные тесты
// seededName не знают, подключён ли стор к разметке.
test("язык: посевные категории и статусы переведены во всех местах", async ({ page }) => {
  // seedDb строго до withMock: сид кладётся в localStorage, откуда его
  // подхватывает tauri-mock.js при загрузке.
  await seedDb(page, {
    tasks: [
      { id: "t-cat", title: "task with category", status: "InProgress", priority: "Medium",
        category: "Work", tags: [], hidden: false, subtasks: [],
        created_at: "2026-07-20T10:00:00Z", updated_at: "2026-07-20T10:00:00Z" },
    ],
    categoryDistribution: [{ category: "Work", count: 3 }, { category: "Health", count: 1 }],
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  // Настройки: имена в полях переименования. Посевные показываются
  // переведёнными и потому заблокированы — иначе правка соседнего поля
  // записала бы перевод в БД поверх русского оригинала.
  // inputValue(), а не селектор [value=…]: Svelte выставляет значение
  // свойством, и в HTML-атрибуте его нет — селектор молча ничего не найдёт.
  await page.locator(".settings-tab", { hasText: "Categories" }).click();
  const catInputs = page.locator("section:has(.section-title) .rule-row input:not([type=color])");
  const catNames = await catInputs.evaluateAll(
    els => els.map(e => (e as HTMLInputElement).value).filter(Boolean));
  expect(catNames).toContain("Work");
  expect(catNames).not.toContain("Работа");

  // Посевные заблокированы: поле показывает перевод, а уходит в БД то же
  // значение — разрешённая правка записала бы английский поверх оригинала.
  await expect(catInputs.nth(catNames.indexOf("Work"))).toBeDisabled();

  // Статусы — на той же вкладке, секцией ниже (SECTION_TAB: обе → "tasks")
  expect(catNames).toContain("In progress");
  expect(catNames).not.toContain("В работе");

  // Задачи: чип категории в списке и колонка канбана
  await page.getByRole("button", { name: /^Tasks$/ }).click();
  await expect(page.locator(".task-row", { hasText: "task with category" })).toContainText("Work");

  await page.getByRole("button", { name: /^Board$/ }).click();
  await expect(page.locator(".column-title", { hasText: "In progress" })).toBeVisible();
  await expect(page.locator(".column-title", { hasText: "В работе" })).toHaveCount(0);

  // Модалка задачи: выпадающие списки категории и статуса
  await page.getByRole("button", { name: /^List$/ }).click();
  await page.locator(".task-row", { hasText: "task with category" }).dblclick();
  const modal = page.locator(".modal, dialog").first();
  // Пункты существуют только у раскрытого списка (свой Select вместо родного
  // <select>), поэтому каждый открывается отдельно.
  await modal.getByRole("button", { name: "Category", exact: true }).click();
  await expect(page.getByRole("option", { name: "Work", exact: true })).toBeVisible();
  await expect(page.getByRole("option", { name: "Работа" })).toHaveCount(0);
  await page.keyboard.press("Escape");

  await modal.getByRole("button", { name: "Status", exact: true }).click();
  await expect(page.getByRole("option", { name: "In progress", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");

  // Дашборд: легенда пончика «Выполнено по категориям»
  await page.getByRole("button", { name: /^Dashboard$/ }).click();
  const donut = page.locator(".legend").first();
  await expect(donut).toContainText("Work");
  await expect(donut).not.toContainText("Работа");
});

// Подсказка календаря выполненных задач звучала как «20 июл. — empty»:
// половина строки переводилась через t(), а дата форматировалась жёстко
// зашитой "ru-RU".
test("язык: дата в подсказке календаря идёт за языком интерфейса", async ({ page }) => {
  // Без единой выполненной задачи календарь показывает «Нет данных»,
  // и наводить будет не на что. seedDb — строго до withMock.
  await seedDb(page, {
    tasks: [
      { id: "t-done", title: "done task", status: "Done", priority: "Medium",
        category: "Work", tags: [], hidden: true, subtasks: [],
        created_at: "2026-07-20T10:00:00Z", updated_at: "2026-07-20T10:00:00Z",
        completed_at: new Date().toISOString() },
    ],
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Настройки" }).click();
  await pickSetting(page, "Язык", "English");

  await page.getByRole("button", { name: /^Dashboard$/ }).click();
  await page.locator(".cal-cell:not(.lead)").last().hover();

  const tip = page.locator(".cal-tip-head");
  await expect(tip).toBeVisible();
  // Месяц в дате — единственная кириллица, которая тут может остаться:
  // остальное («empty») уже шло через t().
  await expect(tip).not.toContainText(/[а-яА-ЯёЁ]/);
});

// v0.9.48: разметка `[ ] ` спрятана виджетом, поэтому пользователь стирает
// подзадачу как обычный текст — и раньше упирался в невидимые скобки: строка
// оставалась на экране (пустая, с чекбоксом), а в данных её уже не было.
test("чек-лист: подзадача стирается текстом, пустая строка не остаётся", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [
        { id: "s1", task_id: "t1", title: "собрать цифры", done: false, position: 0 },
        { id: "s2", task_id: "t1", title: "лишняя строка", done: false, position: 1 },
      ],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  const editor = page.locator(".checklist-editor");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(2);

  // Стираем подзадачу с конца — так её удаляет пользователь. Ровно столько
  // нажатий, сколько букв: строка обязана исчезнуть вместе с последней из
  // них, без добивания невидимых скобок.
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");
  for (let i = 0; i < "лишняя строка".length; i++) {
    await page.keyboard.press("Backspace");
  }

  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(1);
  expect(await editor.locator(".cm-line").count()).toBe(1);

  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title))
  ).toEqual(["собрать цифры"]);

  // Backspace внутри строки обязан остаться обычным удалением символа.
  // Без этой проверки достаточно было бы сносить строку из любой позиции —
  // тест выше прошёл бы, а редактор стал бы непригоден для правки текста.
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.press("Backspace");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(1);
  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title))
  ).toEqual(["собрать цифр"]);
});

// v0.9.49: пользователь показал скриншотом пустые строки в чек-листе —
// Enter и Shift+Enter, после которых он не начал печатать. Они видны на
// экране, но parseChecklist их выбрасывает: сохранено меньше, чем показано.
test("чек-лист: пустые строки исчезают при уходе фокуса", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "собрать цифры", done: false, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  const editor = page.locator(".checklist-editor");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");

  // Shift+Enter даёт такую же подзадачу, как Enter (проверено в браузере:
  // defaultKeymap и так шлёт его в Enter, но привязка объявлена явно, чтобы
  // поведение не держалось на его внутренней детали).
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.insertText("вторая");
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(2);

  // Мусор со скриншота: Enter и Shift+Enter, после которых не начали печатать
  await page.keyboard.press("Enter");
  await page.keyboard.press("Shift+Enter");
  await expect(editor.locator(".cm-line")).toHaveCount(4);

  // Уходим из редактора — пустые строки подчищаются
  await page.locator("input").first().click();
  await expect(editor.locator(".cm-line")).toHaveCount(2);
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(2);

  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title))
  ).toEqual(["собрать цифры", "вторая"]);
});

// v0.9.50: два бага, найденные пользователем. Enter на пустой строке давал
// строку БЕЗ чекбокса (newSubtaskLine выходил при пустой текущей строке, и
// комбинация проваливалась в defaultKeymap). Ctrl+Backspace шёл мимо своего
// Backspace и выедал скобки изнутри — в строке оставался видимый огрызок «[ ».
test("чек-лист: Enter на пустой строке даёт подзадачу, Ctrl+Backspace не обнажает разметку", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "первая", done: false, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  const editor = page.locator(".checklist-editor");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");

  // Enter на пустой строке: чекбокс обязан быть у КАЖДОЙ строки — в чек-листе
  // строк без подзадач не бывает. Считаем именно чекбоксы: строк столько же,
  // а вот виджета у неразмеченной строки раньше не было.
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  await expect(editor.locator(".cm-line")).toHaveCount(3);
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(3);

  // Ctrl+Backspace: удаление слова не должно оставлять огрызок разметки.
  // Проверяем сам текст строки — огрызок «[ » виден пользователю именно там.
  await page.keyboard.insertText("вторая");
  await page.keyboard.press("Control+Backspace");
  await page.keyboard.press("Control+Backspace");
  const texts = await editor.locator(".cm-line").allTextContents();
  expect(texts.join("|"), "разметка обнажилась").not.toMatch(/[[\]]/);

  await page.locator("input").first().click();
  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title))
  ).toEqual(["первая"]);
});

// v0.9.51: Ctrl+Enter принадлежит окну («сохранить»), но в редакторе не был
// привязан — проваливался в defaultKeymap и вставлял пустую строку БЕЗ
// разметки. Список ломался, а сохранение при этом не срабатывало.
test("чек-лист: Ctrl+Enter сохраняет слот, а не вставляет пустую строку", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Отчёт", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "первая", done: false, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
    quickMode: "pinned", pinnedKind: "task", pinnedId: "t1",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  const editor = page.locator(".checklist-editor");
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.press("ControlOrMeta+Enter");
  await page.keyboard.press("ControlOrMeta+Enter");

  // Ни одной новой строки: комбинация не редактирует текст вообще
  await expect(editor.locator(".cm-line")).toHaveCount(1);
  await expect(editor.locator(".cm-sub-checkbox")).toHaveCount(1);

  // И при этом делает то, ради чего нажата: событие всплывает до окна,
  // которое сохраняет слот. Без этого правка комбинации «починила» бы
  // список ценой потери сохранения.
  await expect(page.locator(".pin-saved")).toHaveCount(1);
});

// v0.9.52: баг из боевого использования. У повторяющейся задачи с чек-листом
// пользователь отметил пару подзадач и нажал «выполнить» — задача уехала на
// завтра, но отметки остались. Два разных дефекта в одном сценарии:
// (1) отложенная запись панели прилетала ПОСЛЕ сброса и возвращала отметки в
// БД; (2) панель держит свой текст отдельно от стора, поэтому даже при
// корректной БД на экране оставались галочки.
test("повтор: выполнение сбрасывает чек-лист и в БД, и на экране", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Ежедневная", description: "",
      status: "Todo", priority: "Medium", category: "Work",
      deadline: new Date(Date.now() + 86400000).toISOString(),
      tags: [], recurrence: "Daily", hidden: false, sort_order: 1,
      subtasks: [
        { id: "s1", task_id: "t1", title: "раз", done: false, position: 0 },
        { id: "s2", task_id: "t1", title: "два", done: false, position: 1 },
      ],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
  });
  await withMock(page);
  await page.goto("/");

  // Панель подзадач раскрывается чипом; первый клик может уйти до готовности
  // строки, поэтому ждём редактор и при необходимости кликаем ещё раз.
  await page.waitForSelector(".chip-sub");
  await page.locator(".chip-sub").first().click();
  await page.waitForSelector(".task-sub-panel .checklist-editor", { timeout: 5000 })
    .catch(async () => {
      await page.locator(".chip-sub").first().click();
      await page.waitForSelector(".task-sub-panel .checklist-editor");
    });

  const editor = page.locator(".task-sub-panel .checklist-editor");
  await editor.locator(".cm-sub-checkbox").first().click();

  // Выполняем СРАЗУ, не дожидаясь debounce: именно так это делает человек,
  // и именно здесь отложенная запись обгоняла сброс.
  await page.locator(".task-check").first().click();

  await expect.poll(() => page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.done))
  ).toEqual([false, false]);

  // Ждём заведомо дольше debounce панели (600 мс) и проверяем ЕЩЁ РАЗ:
  // без этого шага тест ловит момент до того, как отложенная запись успела
  // прилететь, и гонка остаётся недоказанной — проверено поломкой (отметка
  // возвращалась в БД на ~700 мс).
  await page.waitForTimeout(1200);
  expect(await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.done)),
    "отложенная запись вернула отметки после сброса").toEqual([false, false]);

  // Экран обязан совпасть с данными: панель не должна показывать отметки,
  // которых в БД уже нет.
  await expect(editor.locator(".cm-sub-checkbox:checked")).toHaveCount(0);
});

// Вторая половина той же правки (v0.9.52): выброс кэша панели при завершении
// обязан идти ПОСЛЕ записи незаписанной правки. Иначе правка уходит вместе с
// кэшем — и теряется не текст, а сама подзадача: в БД остаётся пустой список.
test("выполнение не теряет незаписанную правку чек-листа", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "Разовая", description: "",
      status: "Todo", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: false, sort_order: 1,
      subtasks: [{ id: "s1", task_id: "t1", title: "старое", done: false, position: 0 }],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
  });
  await withMock(page);
  await page.goto("/");

  await page.waitForSelector(".chip-sub");
  await page.locator(".chip-sub").first().click();
  await page.waitForSelector(".task-sub-panel .checklist-editor", { timeout: 5000 })
    .catch(async () => {
      await page.locator(".chip-sub").first().click();
      await page.waitForSelector(".task-sub-panel .checklist-editor");
    });

  await page.locator(".task-sub-panel .checklist-editor").click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.insertText(" ПРАВКА");
  // Выполняем сразу, не дожидаясь debounce
  await page.locator(".task-check").first().click();
  await page.waitForTimeout(1200);

  expect(await page.evaluate(() =>
    JSON.parse(localStorage.getItem("__mock_db")!).tasks[0].subtasks.map((s: any) => s.title)),
    "правка потерялась вместе с кэшем панели").toEqual(["старое ПРАВКА"]);
});

// v0.9.53: клик по задаче в Календаре/Дашборде/«Сегодня» уводил на экран
// Задач (`activeView = "tasks"` + requestFocus). Пользователь смотрел неделю,
// кликал задачу — и оказывался в другом разделе, откуда надо возвращаться
// вручную. Теперь задача открывается на месте.
test("календарь: клик по задаче открывает её, не уводя с календаря", async ({ page }) => {
  const due = new Date();
  due.setHours(12, 0, 0, 0);
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "задача с дедлайном", description: "",
      status: "Todo", priority: "Medium", category: "Work",
      deadline: due.toISOString(),
      tags: [], recurrence: null, hidden: false, sort_order: 1, subtasks: [],
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Календарь" }).click();
  await page.locator(".task-chip").first().click();

  // Задача открыта на правку — и календарь никуда не делся
  await expect(page.getByRole("button", { name: "Сохранить", exact: true })).toBeVisible();
  await expect(page.locator(".page-head")).toContainText("Календарь");
  await expect(page.locator(".task-chip")).toHaveCount(1);

  // Закрытие возвращает ровно туда, где были: раздел не переключался
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Сохранить", exact: true })).toHaveCount(0);
  await expect(page.locator(".page-head")).toContainText("Календарь");
});

// Та же правка на Дашборде, где кликают ВЫПОЛНЕННЫЕ задачи из попапа дня.
// Их надо открывать read-only: править дедлайн и повтор у сделанного незачем
// (правило из v0.9.04, оно и живёт теперь в общем TaskOpener).
test("дашборд: выполненная задача из попапа дня открывается read-only на месте", async ({ page }) => {
  await seedDb(page, {
    tasks: [{
      id: "t1", title: "давно сделанная", description: "",
      status: "Done", priority: "Medium", category: "Work", deadline: null,
      tags: [], recurrence: null, hidden: true, sort_order: 1, subtasks: [],
      completed_at: new Date().toISOString(),
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
    }],
    notes: [], projects: [],
  });
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Дашборд" }).click();
  await page.locator(".cal-cell:not(.lead)").last().click();
  await page.getByText("давно сделанная").first().click();

  // Read-only: кнопки сохранения нет, а раздел прежний
  await expect(page.getByText("давно сделанная").first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Сохранить", exact: true })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Дашборд" })).toBeVisible();
});

// ===== v0.9.54: единый стиль переключателей =====

// Смысл набора: раньше один и тот же элемент был реализован трижды —
// заливка в Задачах, подчёркивание в Настройках, мягкая подсветка в Календаре
// и Дашборде. Проверяем не «как выглядит», а что реализация ОДНА: у всех
// переключателей общая рамка .seg и общий признак выбранного .active.

test("переключатели: во всех разделах один компонент .seg", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], projects: [],
    appUsage: [{ app: "Firefox", minutes: 92 }],
  });
  await withMock(page);
  await page.goto("/");

  // Задачи — две группы в шапке
  await expect(page.locator(".page-head .seg")).toHaveCount(2);

  // Календарь — режим месяц/неделя
  await page.getByRole("button", { name: "Календарь" }).click();
  const cal = page.locator(".page-head .seg");
  await expect(cal.locator("button.active")).toHaveText("Месяц");
  await page.getByRole("button", { name: "Неделя" }).click();
  await expect(cal.locator("button.active")).toHaveText("Неделя");

  // Дашборд — период приложений
  await page.getByRole("button", { name: "Дашборд" }).click();
  const apps = page.locator(".apps-head .seg");
  await expect(apps.locator("button.active")).toHaveText("Сегодня");
  await apps.getByRole("button", { name: "Неделя" }).click();
  await expect(apps.locator("button.active")).toHaveText("Неделя");

  // Настройки — вкладки разделов
  await page.getByRole("button", { name: "Настройки" }).click();
  await expect(page.locator(".settings-tabs.seg")).toHaveCount(1);
  await expect(page.locator(".settings-tabs button.active")).toHaveText("Общее");
});

// Чипы смарт-списков носят класс .active-toggle — то же имя, что у снятых
// переключателей, но это другой элемент: он красится в --bg-hover, а не в
// акцент. Тест держит границу: механическая замена по имени класса его уронит.
test("переключатели: чипы смарт-списков не превратились в .seg", async ({ page }) => {
  await withMock(page);
  await page.goto("/");

  const chip = page.locator(".smart-list-chip", { hasText: "Все" });
  await expect(chip).toHaveClass(/active-toggle/);
  // И сам контейнер, и всё внутри: .seg на любом из них означает, что чипы
  // затянуло в общий переключатель.
  await expect(page.locator(".smart-lists.seg, .smart-lists .seg")).toHaveCount(0);

  // Отличие не только в разметке: выбранный чип красится в --bg-hover, а
  // сегмент — в акцент белым текстом. Совпадение цвета текста с белым значило
  // бы, что чип получил вид сегмента.
  const color = await chip.evaluate(el => getComputedStyle(el).color);
  expect(color).not.toBe("rgb(255, 255, 255)");
});


// Зависимости задач (v0.9.56). Проверяем весь цикл через UI: назначение
// блокера, запрет выполнения, разблокировку при закрытии блокера.
//
// Строки ищем по .task-title, а не по тексту всей строки: у заблокированной
// задачи в разметке есть подпись «Заблокирована: фундамент», и фильтр по
// hasText находил бы обе строки сразу.
function taskByTitle(page: Page, title: string) {
  return page.locator(".task-row").filter({ has: page.locator(".task-title", { hasText: title }) });
}

async function blockWith(page: Page, taskTitle: string, blockerTitle: string) {
  await taskByTitle(page, taskTitle).locator(".task-main").dblclick();
  const modal = page.locator(".modal");
  await pickOption(page, modal, "Добавить блокер...", blockerTitle);
  await modal.getByRole("button", { name: "Отмена" }).click();
}

test("зависимости: заблокированную задачу нельзя выполнить, пока не закрыт блокер", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await createTask(page, "фундамент");
  await createTask(page, "стены");
  await blockWith(page, "стены", "фундамент");

  const walls = taskByTitle(page, "стены");
  await expect(walls).toHaveClass(/blocked/);
  await expect(walls.getByText("Заблокирована: фундамент")).toBeVisible();
  // Причина видна не только глазами: галочка недоступна
  await expect(walls.locator(".task-check")).toBeDisabled();

  // Закрываем блокер — «стены» освобождаются
  await taskByTitle(page, "фундамент").locator(".task-check").click();
  await expect(walls).not.toHaveClass(/blocked/);
  await expect(walls.locator(".task-check")).toBeEnabled();
  await walls.locator(".task-check").click();
  await expect(taskByTitle(page, "стены")).toHaveCount(0);
});

// Решение пользователя: Корзина мягкая, поэтому блокер в ней не блокирует,
// но связь жива и возвращается вместе с задачей при восстановлении.
test("зависимости: блокер в Корзине не блокирует, но связь возвращается при восстановлении", async ({ page }) => {
  await withMock(page);
  await page.goto("/");
  await createTask(page, "фундамент");
  await createTask(page, "стены");
  await blockWith(page, "стены", "фундамент");
  await expect(taskByTitle(page, "стены")).toHaveClass(/blocked/);

  await rowMenu(page, "фундамент", "Удалить");
  await expect(taskByTitle(page, "стены")).not.toHaveClass(/blocked/);

  // Восстановили — блокировка вернулась
  await page.getByRole("button", { name: "Корзина" }).click();
  await page.getByRole("button", { name: "Восстановить" }).first().click();
  await page.getByRole("button", { name: "Активные" }).click();
  await expect(taskByTitle(page, "стены")).toHaveClass(/blocked/);
});

// ИИ-классификация приложений (v0.9.62): модель предлагает правила, в настройки
// они попадают только по явному клику. Проверяем весь путь, включая главное —
// что до подтверждения в списке правил ничего не появилось.
test("ИИ-правила приложений: предложение не меняет настройки до подтверждения", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], projects: [],
    settings: { onboarding_complete: true, ai_provider: "openai", openai_key: "k" },
    windowTracking: "Hyprland",
    appUsage: [{ app: "jetbrains-idea", minutes: 120 }, { app: "obscure-tool", minutes: 30 }],
    aiAppRules: [{ pattern: "jetbrains-*", category: "Work" }],
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();

  await page.getByRole("button", { name: "Определить категории через ИИ" }).click();

  const suggestions = page.locator(".rule-suggestions");
  await expect(suggestions.locator("code", { hasText: "jetbrains-*" })).toBeVisible();
  // Ключевое: предложение ещё не правило. В самом списке правил пусто.
  await expect(page.locator(".rule-row input[placeholder*='класс окна']")).toHaveCount(0);

  await suggestions.getByRole("button", { name: "Добавить отмеченные" }).click();
  await expect(page.locator(".rule-row input[placeholder*='класс окна']")).toHaveValue("jetbrains-*");
  await expect(suggestions).toHaveCount(0);
});

// Снятая галочка означает «не добавлять»: пользователь отбирает предложенное,
// а не получает всё оптом.
test("ИИ-правила приложений: снятая галочка не попадает в правила", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], projects: [],
    settings: { onboarding_complete: true, ai_provider: "openai", openai_key: "k" },
    windowTracking: "Hyprland",
    appUsage: [{ app: "jetbrains-idea", minutes: 120 }, { app: "steam_app_570", minutes: 90 }],
    aiAppRules: [
      { pattern: "jetbrains-*", category: "Work" },
      { pattern: "steam_app_*", category: "Home" },
    ],
  });
  await withMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Настройки" }).click();
  await page.locator(".settings-tab", { hasText: "Категории" }).click();
  await page.getByRole("button", { name: "Определить категории через ИИ" }).click();

  const suggestions = page.locator(".rule-suggestions");
  await expect(suggestions.locator(".suggestion-row")).toHaveCount(2);
  await suggestions.locator(".suggestion-row", { hasText: "steam_app_*" }).locator("input").uncheck();
  await suggestions.getByRole("button", { name: "Добавить отмеченные" }).click();

  const patterns = page.locator(".rule-row input[placeholder*='класс окна']");
  await expect(patterns).toHaveCount(1);
  await expect(patterns).toHaveValue("jetbrains-*");
});


// v0.9.65: голосовой ввод требует и модели, и бинарника whisper-cli. Пока их нет,
// кнопки не должно быть вовсе — не отключённой, а отсутствующей: это тот же
// capability detection, что у трекинга окон и кнопок в уведомлениях. Отключённая
// кнопка задавала бы вопрос, на который из этого места не ответить.
test("голос: без модели кнопки микрофона нет", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [] }); // voiceAvailable не задан
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();

  await expect(page.locator(".format-toolbar")).toBeVisible();
  await expect(page.locator(".voice-btn")).toHaveCount(0);
});

test("голос: надиктованный текст попадает в заметку", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [],
    voiceAvailable: true,
    voiceText: "купить хлеб и молоко",
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();

  const mic = page.locator(".format-toolbar .voice-btn");
  await expect(mic).toBeVisible();

  // Первый клик — запись пошла: состояние видно по классу, а не только по тексту
  // подсказки, иначе пользователь не понимает, идёт ли запись.
  await mic.click();
  await expect(mic).toHaveClass(/recording/);

  // Второй — распознавание и вставка
  await mic.click();
  await expect(mic).not.toHaveClass(/recording/);
  await expect(page.locator(".cm-content")).toContainText("купить хлеб и молоко");
});

// Диктовка — это ввод текста, а не замена содержимого: она обязана уважать
// каретку ровно так же, как печать с клавиатуры.
test("голос: текст вставляется в позицию каретки, а не затирает заметку", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [],
    voiceAvailable: true,
    voiceText: "вставка",
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await fillNoteEditor(page, "начало конец");

  // ставим каретку между словами: 7 нажатий влево от конца («конец» + пробел)
  // каретка между словами: «конец» — 5 символов, ещё шаг через пробел
  await page.keyboard.press("End");
  for (let i = 0; i < 6; i++) await page.keyboard.press("ArrowLeft");

  const mic = page.locator(".format-toolbar .voice-btn");
  await mic.click();
  await mic.click();

  // существующий текст цел, надиктованное — внутри него
  await expect(page.locator(".cm-content")).toContainText("начало вставка конец");
});

// Быстрый слот — второе место, где диктовка нужна: сценарий «сказал и забыл».
// Механика вставки там другая (обычная textarea, не CodeMirror), поэтому
// проверяется отдельно.
test("голос: диктовка в быстрый слот вставляет текст в textarea", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], projects: [],
    quickMode: "note",
    voiceAvailable: true,
    voiceText: "мысль на бегу",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  const field = page.locator("textarea");
  await field.fill("было: ");

  const mic = page.locator(".field-with-voice .voice-btn");
  await mic.click();
  await mic.click();

  await expect(field).toHaveValue("было: мысль на бегу");
});

// v0.9.66: диктовка по Ctrl+Shift+D — второй вход в ту же запись, что и кнопка.
// Состояние общее (lib/voice.svelte.ts), поэтому хоткей и кнопка не могут
// разойтись в том, идёт запись или нет.
test("голос: хоткей Ctrl+Shift+D диктует в редактор заметок", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [],
    voiceAvailable: true,
    voiceText: "надиктовано хоткеем",
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await fillNoteEditor(page, "начало");

  const mic = page.locator(".format-toolbar .voice-btn");

  // первое нажатие — запись пошла, и это видно на кнопке: состояние общее
  await page.keyboard.press("Control+Shift+D");
  await expect(mic).toHaveClass(/recording/);

  // второе — распознавание и вставка в каретку. fillNoteEditor оставляет каретку
  // на новой пустой строке, поэтому пробел слева не добавляется — в начале строки
  // он не нужен; обе строки целы.
  await page.keyboard.press("Control+Shift+D");
  await expect(mic).not.toHaveClass(/recording/);
  await expect(page.locator(".cm-content")).toContainText("начало");
  await expect(page.locator(".cm-content")).toContainText("надиктовано хоткеем");
});

// Без модели хоткей не наш: он не должен ни начинать запись, ни съедать комбинацию.
test("голос: без модели хоткей не начинает запись", async ({ page }) => {
  await seedDb(page, { tasks: [], notes: [] }); // voiceAvailable не задан
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки" }).click();
  await page.getByRole("button", { name: "+ Новая заметка" }).click();
  await fillNoteEditor(page, "текст");

  await page.keyboard.press("Control+Shift+D");

  await expect(page.locator(".voice-btn")).toHaveCount(0);
  await expect(page.locator(".cm-content")).toContainText("текст");
});

test("голос: хоткей в быстром слоте вставляет в поле с кареткой", async ({ page }) => {
  await seedDb(page, {
    tasks: [], notes: [], projects: [],
    quickMode: "note",
    voiceAvailable: true,
    voiceText: "голосом",
  });
  await withMock(page);
  await page.goto("/quick-task.html");

  const field = page.locator("textarea");
  await field.fill("было:");
  await field.focus();

  await page.keyboard.press("Control+Shift+D");
  await page.keyboard.press("Control+Shift+D");

  await expect(field).toHaveValue("было: голосом");
});

// --- v0.9.77: клавиатурная навигация по спискам ---

// Сид с тремя задачами в предсказуемом порядке: createTask() кладёт их через UI,
// но так порядок зависит от sort_order, а тесты курсора обязаны знать, какая
// строка первая.
function navTasksDb(extra: object = {}) {
  const base = (i: number, title: string) => ({
    id: `t${i}`, title, description: null, priority: "Medium", category: "Работа",
    status: "Todo", deadline: null, tags: [], completed_at: null, recurrence: "None",
    hidden: false, deleted_at: null, project_id: null, scheduled_at: null,
    scheduled_mins: null, sort_order: i, subtasks: [],
    created_at: "2026-01-01T10:00:00Z", updated_at: "2026-01-01T10:00:00Z",
  });
  return {
    tasks: [base(1, "первая"), base(2, "вторая"), base(3, "третья")],
    notes: [], projects: [], ...extra,
  };
}

test("клавиатура: j/k двигают курсор по списку задач", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  const rows = page.locator(".task-row");
  await expect(rows).toHaveCount(3);
  // До первого нажатия курсора нет — экран не должен «подсвечиваться» сам собой.
  await expect(page.locator(".task-row.kb-focused")).toHaveCount(0);

  await page.keyboard.press("j");
  await expect(rows.nth(0)).toHaveClass(/kb-focused/);
  await page.keyboard.press("j");
  await expect(rows.nth(1)).toHaveClass(/kb-focused/);
  // Курсор ровно один: старая строка обязана его отдать.
  await expect(page.locator(".task-row.kb-focused")).toHaveCount(1);

  await page.keyboard.press("k");
  await expect(rows.nth(0)).toHaveClass(/kb-focused/);

  // Стрелки работают наравне с буквами.
  await page.keyboard.press("ArrowDown");
  await expect(rows.nth(1)).toHaveClass(/kb-focused/);

  // На краях курсор упирается, а не заворачивается.
  await page.keyboard.press("k");
  await page.keyboard.press("k");
  await expect(rows.nth(0)).toHaveClass(/kb-focused/);

  // Escape снимает курсор.
  await page.keyboard.press("Escape");
  await expect(page.locator(".task-row.kb-focused")).toHaveCount(0);
});

// ГЛАВНАЯ регрессия версии: пока каретка в поле, буквы обязаны печататься.
// Без проверки document.activeElement композер стал бы неработоспособен —
// каждая "j" двигала бы курсор вместо ввода текста.
test("клавиатура: в поле ввода j печатается, а не двигает курсор", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  const composer = page.locator(".composer-input");
  await composer.click();
  await page.keyboard.type("jkj");

  await expect(composer).toHaveValue("jkj");
  await expect(page.locator(".task-row.kb-focused")).toHaveCount(0);
});

// Enter повторяет одинарный клик — правит название прямо в строке, а не открывает
// карточку. Иначе клавиатура и мышь делали бы одно и то же двумя разными вещами.
test("клавиатура: Enter правит название задачи под курсором", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  await page.keyboard.press("j");
  await page.keyboard.press("j");
  await page.keyboard.press("Enter");

  // Карточка не открылась — правка идёт в строке.
  await expect(page.locator(".modal")).toHaveCount(0);
  const editor = page.locator(".task-title-edit");
  await expect(editor).toBeFocused();
  await expect(editor).toHaveValue("вторая");

  await editor.fill("вторая переименованная");
  await page.keyboard.press("Enter");
  await expect(page.locator(".task-title-edit")).toHaveCount(0);
  await expect(page.getByText("вторая переименованная")).toBeVisible();
});

test("клавиатура: пробел выполняет задачу под курсором", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  await page.keyboard.press("j");
  await page.keyboard.press(" ");

  // Выполненная уходит из активного списка в Историю.
  await expect(page.locator(".task-row", { hasText: "первая" })).toHaveCount(0);
  await expect(page.locator(".task-row")).toHaveCount(2);
  // Курсор остаётся на позиции — там теперь следующая задача, продолжать удобно.
  await expect(page.locator(".task-row").nth(0)).toHaveClass(/kb-focused/);
});

// Запрет на выполнение заблокированной живёт в бэкенде (v0.9.56) и в UI
// (disabled на галочке). Клавиатура не должна стать третьим путём в обход обоих.
test("клавиатура: пробел не выполняет заблокированную задачу", async ({ page }) => {
  await seedDb(page, navTasksDb({
    taskDeps: [{ task_id: "t2", blocker_id: "t1" }],
  }));
  await withMock(page);
  await page.goto("/");

  const blocked = page.locator(".task-row", { hasText: "вторая" });
  await expect(blocked).toHaveClass(/blocked/);

  await page.keyboard.press("j");
  await page.keyboard.press("j");
  await expect(blocked).toHaveClass(/kb-focused/);
  await page.keyboard.press(" ");

  // Задача на месте и по-прежнему заблокирована.
  await expect(page.locator(".task-row", { hasText: "вторая" })).toHaveCount(1);
  await expect(page.locator(".task-row")).toHaveCount(3);

  // Ключевая проверка. Одного «задача не выполнилась» мало: мок зеркалит запрет
  // бэкенда (v0.9.56), поэтому она не выполнилась бы и без фронтового запрета —
  // но пользователь получил бы ошибку в .alert. Клавиатура обязана вести себя
  // как disabled-галочка: молча ничего не делать.
  await expect(page.locator(".alert")).toHaveCount(0);
});

test("клавиатура: Delete отправляет задачу под курсором в Корзину", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  await page.keyboard.press("j");
  await page.keyboard.press("Delete");

  await expect(page.locator(".task-row", { hasText: "первая" })).toHaveCount(0);

  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(page.locator(".task-row", { hasText: "первая" })).toBeVisible();
});

test("клавиатура: в заметках j/k двигают курсор, Enter открывает", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [
      { id: "n1", title: "альфа", content: "текст альфы", tags: [], pinned: false,
        linked_task_id: null, project_id: null, reminder_at: null, deleted_at: null,
        created_at: "2026-01-01T10:00:00Z", updated_at: "2026-01-02T10:00:00Z" },
      { id: "n2", title: "бета", content: "текст беты", tags: [], pinned: false,
        linked_task_id: null, project_id: null, reminder_at: null, deleted_at: null,
        created_at: "2026-01-01T10:00:00Z", updated_at: "2026-01-01T10:00:00Z" },
    ],
  });
  await withMock(page);
  await page.goto("/");
  await page.locator(".nav").getByRole("button", { name: "Заметки", exact: true }).click();

  const rows = page.locator(".note-row");
  await expect(rows).toHaveCount(2);

  await page.keyboard.press("j");
  await expect(rows.nth(0)).toHaveClass(/kb-focused/);
  await page.keyboard.press("j");
  await expect(rows.nth(1)).toHaveClass(/kb-focused/);

  // Движение курсора НЕ открывает заметку: иначе каждое j/k запускало бы
  // загрузку и цикл отложенного сохранения.
  await expect(page.locator(".title-input")).toHaveCount(0);

  await page.keyboard.press("Enter");
  await expect(page.locator(".title-input")).toHaveValue("бета");
});

// --- v0.9.78: заблокированные задачи — смарт-список и «разблокирует N» ---

test("смарт-список «Заблокированные» отбирает только задачи с блокерами", async ({ page }) => {
  await seedDb(page, navTasksDb({
    taskDeps: [{ task_id: "t2", blocker_id: "t1" }],
  }));
  await withMock(page);
  await page.goto("/");

  await expect(page.locator(".task-row")).toHaveCount(3);

  const chip = page.locator(".smart-list-chip", { hasText: "Заблокированные" });
  await chip.click();
  await expect(chip).toHaveClass(/active-toggle/);

  await expect(page.locator(".task-row")).toHaveCount(1);
  await expect(page.locator(".task-row", { hasText: "вторая" })).toBeVisible();

  // Повторный клик по чипу снимает фильтр — как у остальных смарт-списков.
  await chip.click();
  await expect(page.locator(".task-row")).toHaveCount(3);
});

test("бейдж «разблокирует N» появляется у блокера и исчезает после его выполнения", async ({ page }) => {
  // t1 держит и t2, и t3 — счётчик должен быть 2, а не «есть/нет».
  await seedDb(page, navTasksDb({
    taskDeps: [
      { task_id: "t2", blocker_id: "t1" },
      { task_id: "t3", blocker_id: "t1" },
    ],
  }));
  await withMock(page);
  await page.goto("/");

  // Фильтр по .task-title, а не по тексту всей строки: у зависимых в строке есть
  // «Заблокирована: первая», и hasText поймал бы все три.
  const blocker = page.locator(".task-row").filter({ has: page.locator(".task-title", { hasText: "первая" }) });
  await expect(blocker.locator(".task-unblocks")).toHaveText("разблокирует 2");

  // Бейдж только у блокера: у тех, кто никого не держит, его нет.
  await expect(page.locator(".task-unblocks")).toHaveCount(1);

  await blocker.locator(".task-check").click();

  // Блокер ушёл в Историю, зависимые разблокированы, бейджа больше нет.
  await expect(page.locator(".task-unblocks")).toHaveCount(0);
  await expect(page.locator(".task-row.blocked")).toHaveCount(0);
});

test("счётчик «разблокирует N» не зависит от фильтра по проекту", async ({ page }) => {
  // t1 держит t2 и t3, но t1 лежит в проекте, а зависимые — нет. При фильтре
  // «по проекту» на экране виден только блокер, и счётчик обязан остаться 2:
  // он говорит о самой задаче, а не о том, что сейчас показано.
  await seedDb(page, navTasksDb({
    projects: [{ id: "p1", name: "Проект", color: "#888", target_date: null,
      archived: false, sort_order: 0, task_total: 1, task_done: 0,
      goal_tasks: null, goal_mins: null, goal_period: null,
      goal_done_tasks: 0, goal_done_mins: 0,
      created_at: "2026-01-01T10:00:00Z", updated_at: "2026-01-01T10:00:00Z" }],
    taskDeps: [
      { task_id: "t2", blocker_id: "t1" },
      { task_id: "t3", blocker_id: "t1" },
    ],
  }));
  await withMock(page);
  await page.goto("/");

  // Кладём блокер в проект через карточку задачи.
  await page.locator(".task-row")
    .filter({ has: page.locator(".task-title", { hasText: "первая" }) })
    .locator(".task-main").dblclick();
  await pickOption(page, page.locator(".modal"), "Проект", "Проект");
  await page.locator(".modal").getByRole("button", { name: "Сохранить", exact: true }).click();

  await page.locator(".project-filter").selectOption({ label: "Проект" });
  await expect(page.locator(".task-row")).toHaveCount(1);
  await expect(page.locator(".task-unblocks")).toHaveText("разблокирует 2");
});

// --- v0.9.79: память состояния экранов ---

test("состояние экранов: активный раздел и режим Доска переживают перезапуск", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  await page.getByRole("button", { name: "Доска", exact: true }).click();
  await expect(page.locator(".board")).toBeVisible();

  await page.reload();
  // Раздел тот же и режим тот же — без ручного восстановления контекста.
  await expect(page.getByRole("heading", { name: "Задачи" })).toBeVisible();
  await expect(page.locator(".board")).toBeVisible();
});

test("состояние экранов: раздел Заметки открывается сразу после перезапуска", async ({ page }) => {
  await seedDb(page, {
    tasks: [], projects: [],
    notes: [
      { id: "n1", title: "запомненная", content: "текст", tags: [], pinned: false,
        linked_task_id: null, project_id: null, reminder_at: null, deleted_at: null,
        created_at: "2026-01-01T10:00:00Z", updated_at: "2026-01-02T10:00:00Z" },
    ],
  });
  await withMock(page);
  await page.goto("/");

  await page.locator(".nav").getByRole("button", { name: "Заметки", exact: true }).click();
  await page.locator(".note-row .note-item").first().click();
  await expect(page.locator(".title-input")).toHaveValue("запомненная");

  await page.reload();
  // И раздел, и сама заметка — открыты там же, где закрыли.
  await expect(page.locator(".title-input")).toHaveValue("запомненная");
});

// Главная граница фичи: сохранённое состояние может протухнуть между запусками.
// Молча откатываемся к умолчанию, а не показываем пустой экран под фильтром-призраком.
//
// Смарт-список НЕ удаляется через UI намеренно: `removeSmartList` сам сбрасывает
// активный фильтр, и тест проверял бы этот сброс, а не восстановление. Здесь
// воспроизведён честный случай «состояние пережило запуск, а список — нет»:
// ui_state ссылается на id, которого в базе уже нет.
test("состояние экранов: удалённый смарт-список после перезапуска не ломает экран", async ({ page }) => {
  await seedDb(page, navTasksDb({ smartLists: [] }));
  await page.addInitScript(() =>
    localStorage.setItem("ui_state", JSON.stringify({ view: "tasks", smartListId: "sl-удалённый" })));
  await withMock(page);
  await page.goto("/");

  // Экран рабочий, фильтр молча сброшен на «Все», задачи на месте.
  await expect(page.locator(".task-row")).toHaveCount(3);
  await expect(page.locator(".smart-list-chip", { hasText: "Все" })).toHaveClass(/active-toggle/);
  await expect(page.locator(".smart-list-chip.active-toggle")).toHaveCount(1);
});

test("состояние экранов: битое сохранённое состояние не мешает запуску", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await page.addInitScript(() => localStorage.setItem("ui_state", "{не json"));
  await withMock(page);
  await page.goto("/");

  // Приложение стартует на умолчании, а не падает.
  await expect(page.getByRole("heading", { name: "Задачи" })).toBeVisible();
  await expect(page.locator(".task-row")).toHaveCount(3);
});

test("состояние экранов: Корзина НЕ запоминается — запуск всегда в активных", async ({ page }) => {
  await seedDb(page, navTasksDb());
  await withMock(page);
  await page.goto("/");

  // Корзина должна быть непустой, иначе список вообще не отрисуется и проверка
  // «не открылись в Корзине» стала бы бессодержательной.
  await rowMenu(page, "третья", "Удалить");
  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(page.locator(".task-list.trash")).toBeVisible();

  await page.reload();
  // Открыть приложение сразу в Корзине — дезориентирует, поэтому подвид намеренно
  // не сохраняется.
  await expect(page.locator(".task-list.trash")).toHaveCount(0);
  // Задач снова три: init-скрипт сида выполняется на КАЖДОЙ навигации, включая
  // reload(), и возвращает базу к исходной. Здесь это не мешает — проверяется
  // подвид, а не содержимое базы.
  await expect(page.locator(".task-row")).toHaveCount(3);
});
