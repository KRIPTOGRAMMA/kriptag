// Мок Tauri-бэкенда для E2E: подсовывается через page.addInitScript ДО кода
// приложения. @tauri-apps/api v2 весь ходит через window.__TAURI_INTERNALS__
// (invoke + transformCallback) — реализуем оба, и фронт работает как в Tauri.
//
// БД мока сериализуется в localStorage (__mock_db) на каждой мутации, поэтому
// переживает page.reload(). Тест может сидировать состояние, положив свой
// __mock_db в localStorage init-скриптом, добавленным ПЕРЕД этим файлом.
(() => {
  const now = () => new Date().toISOString();
  const uuid = () =>
    (crypto.randomUUID ? crypto.randomUUID() : String(Math.random()).slice(2));

  const defaultSettings = {
    ai_provider: "none",
    openai_key: "",
    openai_model: "gpt-4o-mini",
    anthropic_key: "",
    anthropic_model: "claude-haiku-4-5-20251001",
    idle_threshold_secs: 300,
    log_interval_secs: 60,
    work_mode: "Light",
    onboarding_complete: true,
    deadline_warn_hours: 24,
    deadline_warn_minutes: 60,
    idle_notify_min_mins: 10,
    pomodoro_work_mins: 25,
    pomodoro_break_mins: 5,
    nudge_after_mins: 90,
    theme_mode: "system",
    color_accent: "",
    color_accent_secondary: "",
    color_bg: "",
    color_bg_secondary: "",
    color_bg_hover: "",
    color_bg_card: "",
    color_text_secondary: "",
    color_text: "",
    color_border: "",
    quiet_until: "",
    context_notifications: true,
    ai_fallback: false,
    openai_in_keyring: false,
    anthropic_in_keyring: false,
    custom_theme_presets: "",
    app_category_rules: "",
    auto_backup_dir: "",
    auto_backup_keep: 7,
    last_auto_backup: "",
    last_auto_backup_error: "",
    morning_digest_time: "",
    show_subtasks_expanded: true,
    keybinds: "",
    global_keybinds: "",
    focus_mode_auto: true,
    track_domains: false,
    // v0.9.32: в e2e язык прибит к русскому намеренно. Тесты ищут элементы
    // по текстам, и это проверка логики, а не перевода — иначе каждая
    // строка словаря ломала бы десятки тестов. Само переключение языка
    // покрыто отдельным тестом, который задаёт language явно.
    language: "ru",
    history_cleanup_months: 0,
  };

  let db;
  try {
    db = JSON.parse(localStorage.getItem("__mock_db") ?? "null");
  } catch {
    db = null;
  }
  if (!db) db = { tasks: [], notes: [], projects: [], settings: { ...defaultSettings } };
  if (!db.projects) db.projects = [];
  // Как и projects строкой выше: сид задаёт только те разделы, которые ему
  // нужны, и db.notes может не быть вовсе. Без этой строки нормализация ниже
  // падала на undefined и обрывала инициализацию мока целиком — экран
  // оставался пустым, а падал при этом посторонний тест про язык.
  if (!db.notes) db.notes = [];
  if (!db.tasks) db.tasks = [];
  for (const p of db.projects) {
    p.goal_period ??= "week";
    p.goal_tasks ??= null;
    p.goal_mins ??= null;
  }
  // Ручной порядок: сид мог не проставить sort_order
  db.tasks.forEach((t, i) => { t.sort_order ??= i + 1; });
  // Идентификаторы заметок: сид пишет только title/content, а настоящий бэкенд
  // выдаёт id при вставке. Без этого всё, что адресует заметку ПО id, молча
  // работает с undefined — так и всплыло в v0.10.20 (клик по источнику ответа
  // переключал раздел, но заметку не открывал: find по undefined не находит).
  db.notes.forEach((n) => {
    n.id ??= uuid();
    n.tags ??= [];
    n.created_at ??= now();
    n.updated_at ??= now();
  });
  // Категории задач: зеркало посева миграции 0015
  if (!db.categories) {
    db.categories = [
      { id: "Work", name: "Работа", color: "#2a78d6", position: 0 },
      { id: "Study", name: "Учёба", color: "#1baf7a", position: 1 },
      { id: "Home", name: "Дом", color: "#eda100", position: 2 },
      { id: "Health", name: "Здоровье", color: "#008300", position: 3 },
      { id: "Other", name: "Другое", color: "#4a3aa7", position: 4 },
    ];
  }
  // Статусы задач: зеркало посева миграции 0029 (v0.9.20)
  if (!db.statuses) {
    db.statuses = [
      { id: "Todo", name: "Todo", color: "#94a3b8", position: 0, is_reserved: true },
      { id: "InProgress", name: "В работе", color: "#2a78d6", position: 1, is_reserved: true },
      { id: "Done", name: "Готово", color: "#1baf7a", position: 2, is_reserved: true },
      { id: "Archived", name: "Архив", color: "#6b7280", position: 3, is_reserved: true },
    ];
  }
  // сид может задать только часть настроек
  db.settings = { ...defaultSettings, ...db.settings };

  const persist = () => localStorage.setItem("__mock_db", JSON.stringify(db));
  persist();

  // Реестр слушателей событий (plugin:event|listen). Колбэки живут в
  // __mockCallbacks под id, выданным transformCallback.
  const callbacks = new Map();
  let nextCallbackId = 1;
  const eventHandlers = new Map(); // event -> Set<callbackId>

  window.__mockEmit = (event, payload) => {
    for (const id of eventHandlers.get(event) ?? []) {
      const cb = callbacks.get(id);
      if (cb) cb({ event, payload, id });
    }
  };

  window.__unknownInvokes = [];

  const findTask = (id) => db.tasks.find((t) => t.id === id);
  // Незакрытые блокеры задачи (v0.9.56). Зеркало OPEN_BLOCKER в Rust: задача
  // в Корзине НЕ блокирует, но сама связь в db.taskDeps остаётся — поэтому
  // восстановление блокера возвращает блокировку.
  const openBlockers = (taskId) =>
    (db.taskDeps ?? [])
      .filter((d) => d.task_id === taskId)
      .map((d) => findTask(d.blocker_id))
      .filter((b) => b && !b.completed_at && !b.hidden && !b.deleted_at)
      .map((b) => ({ id: b.id, title: b.title }));
  // Локальная дата YYYY-MM-DD из ISO-метки (зеркало 'localtime' в SQLite)
  const localDayKey = (iso) => {
    const d = new Date(iso);
    const p = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  };

  const commands = {
    // --- настройки / окружение ---
    get_settings: () => ({ ...db.settings }),
    save_settings: ({ settings }) => {
      db.settings = { ...db.settings, ...settings };
      persist();
    },
    // Тест страницы биндов композитора выставляет window.__forceWayland: это
    // единственный шаг онбординга, которого на не-Wayland быть не должно.
    is_wayland: () => window.__forceWayland === true,
    get_tracking_mode: () => "basic",
    get_window_tracking: () => db.windowTracking ?? null,
    record_input: () => {},
    open_quick_capture: ({ mode }) => {
      db.quickMode = mode;
      persist();
    },
    get_quick_mode: () => db.quickMode ?? "task",
    // v0.9.26: реального буфера в e2e нет — тест кладёт текст в
    // window.__mockClipboard, мок отдаёт его как Rust-команда.
    read_clipboard_text: () => window.__mockClipboard ?? "",

    // v0.9.33: быстрый слот. Зеркалит commands/pinned.rs, включая главное:
    // задача из Корзины (deleted_at) читается как пустой слот, а не как
    // живая запись — иначе хоткей открывал бы выброшенное на правку.
    get_pinned_item: () => {
      const kind = db.pinnedKind;
      const id = db.pinnedId;
      if ((kind !== "task" && kind !== "note") || !id) return null;
      if (kind === "task") {
        const t = db.tasks.find((t) => t.id === id && !t.deleted_at);
        // v0.9.34: чек-лист едет вместе со слотом; у заметки он всегда пуст.
        return t ? { kind, id, title: t.title, text: t.description ?? "", subtasks: t.subtasks ?? [] } : null;
      }
      const n = db.notes.find((n) => n.id === id);
      return n ? { kind, id, title: n.title, text: n.content ?? "", subtasks: [] } : null;
    },
    // v0.9.35: глобальные хоткеи. Список действий зеркалит commands/hotkeys.rs
    // (он же источник правды для регистрации в ОС).
    list_global_actions: () => [
      { id: "quick_task", label: "Быстрая задача", default_combo: "Ctrl+Shift+KeyN" },
      { id: "quick_note", label: "Быстрая заметка", default_combo: "Ctrl+Shift+KeyM" },
      { id: "quick_clip", label: "Заметка из буфера", default_combo: "Ctrl+Shift+KeyB" },
      { id: "quick_pinned", label: "Быстрый слот", default_combo: "Ctrl+Shift+KeyJ" },
    ],
    // Зеркалит validate_combo: пусто и одиночная клавиша без модификатора —
    // отказ (иначе хоткей перехватил бы букву во всей системе).
    validate_global_combo: ({ combo }) => {
      const c = (combo ?? "").trim();
      if (!c) throw "Пустая комбинация";
      if (!c.includes("+")) throw "Нужен хотя бы один модификатор (Ctrl, Shift, Alt)";
    },
    // В моке регистрировать нечего: ОС нет. Пустой список = всё применилось.
    apply_global_hotkeys: () => [],
    set_pinned_item: ({ kind, id }) => {
      const valid = (kind === "task" || kind === "note") && id;
      db.pinnedKind = valid ? kind : "";
      db.pinnedId = valid ? id : "";
      persist();
    },

    // --- задачи ---
    get_tasks: () =>
      [...db.tasks].filter((t) => !t.deleted_at).sort((a, b) => a.sort_order - b.sort_order)
        .map((t) => ({ ...t, blocked_by: openBlockers(t.id) })),
    reorder_tasks: ({ ids }) => {
      // Зеркало бэкенда: та же тройка значений раздаётся по новому порядку
      const byId = new Map(db.tasks.map((t) => [t.id, t]));
      const live = ids.filter((id) => byId.has(id));
      const orders = live.map((id) => byId.get(id).sort_order).sort((a, b) => a - b);
      live.forEach((id, i) => { byId.get(id).sort_order = orders[i]; });
      persist();
    },
    create_task: ({ task }) => {
      const full = {
        id: uuid(),
        description: null,
        deadline: null,
        tags: [],
        completed_at: null,
        recurrence: "None",
        hidden: false,
        deleted_at: null,
        project_id: null,
        scheduled_at: null,
        scheduled_mins: null,
        sort_order: Math.max(0, ...db.tasks.map((t) => t.sort_order)) + 1,
        subtasks: [],
        ...task,
        created_at: now(),
        updated_at: now(),
      };
      db.tasks.push(full);
      persist();
      return { ...full };
    },
    update_task: ({ id, patch }) => {
      const t = findTask(id);
      if (!t) throw `Задача не найдена: ${id}`;
      for (const [k, v] of Object.entries(patch)) {
        if (v === undefined) continue;
        // конвенции бэкенда: пустая строка = снять значение
        if (k === "project_id") t.project_id = v === "" ? null : v;
        else if (k === "scheduled_at") {
          if (v === "") { t.scheduled_at = null; t.scheduled_mins = null; }
          else t.scheduled_at = v;
        } else t[k] = v;
      }
      t.updated_at = now();
      persist();
      return { ...t };
    },
    delete_task: ({ id }) => {
      const t = findTask(id);
      if (!t) throw `Задача не найдена: ${id}`;
      t.deleted_at = now();
      persist();
    },
    get_deleted_tasks: () =>
      db.tasks.filter((t) => t.deleted_at).sort((a, b) => new Date(b.deleted_at) - new Date(a.deleted_at)).map((t) => ({ ...t })),
    restore_task: ({ id }) => {
      const t = findTask(id);
      if (!t) throw `Задача не найдена: ${id}`;
      t.deleted_at = null;
      persist();
    },
    purge_deleted_task: ({ id }) => {
      db.tasks = db.tasks.filter((t) => t.id !== id);
      for (const n of db.notes) if (n.linked_task_id === id) n.linked_task_id = null;
      persist();
    },
    complete_task: ({ id }) => {
      const t = findTask(id);
      if (!t) throw `Задача не найдена: ${id}`;
      // Зеркало бэкенда (v0.9.56): заблокированную задачу выполнить нельзя.
      const blockers = openBlockers(id);
      if (blockers.length > 0) {
        throw `Сначала выполните: ${blockers.map((b) => b.title).join(", ")}.`;
      }
      // Как в Rust: ветка зависит от повтора (v0.9.24 — раньше мок считал
      // любую задачу разовой и не воспроизводил баг с повторами).
      const repeats = t.recurrence && t.recurrence !== "None";
      t.updated_at = now();
      if (repeats) {
        // Повтор не закрывается, а едет на следующий дедлайн и возвращается
        // в Todo из любого статуса; чеклист — план следующего прогона, сброс.
        t.status = "Todo";
        t.hidden = false;
        t.deadline = new Date(Date.now() + 864e5).toISOString();
        for (const s of t.subtasks) s.done = false;
      } else {
        t.status = "Done";
        t.hidden = true;
        t.completed_at = now();
        for (const s of t.subtasks) s.done = true;
      }
      persist();
      return { ...t };
    },
    search_tasks: ({ query }) => {
      const q = query.toLowerCase();
      return db.tasks.filter((t) => !t.deleted_at && t.title.toLowerCase().includes(q));
    },

    // --- проекты ---
    get_projects: () =>
      db.projects.map((p) => ({
        ...p,
        task_total: db.tasks.filter((t) => t.project_id === p.id).length,
        task_done: db.tasks.filter((t) => t.project_id === p.id && t.completed_at).length,
        // упрощение мока: весь прогресс считаем «в текущем периоде»
        goal_done_tasks: db.tasks.filter((t) => t.project_id === p.id && t.completed_at).length,
        goal_done_mins: db.tasks
          .filter((t) => t.project_id === p.id && t.scheduled_at && new Date(t.scheduled_at) <= new Date())
          .reduce((s, t) => s + (t.scheduled_mins ?? 60), 0),
      })),
    create_project: ({ project }) => {
      const full = {
        id: uuid(),
        color: "",
        target_date: null,
        archived: false,
        goal_tasks: null,
        goal_mins: null,
        goal_period: "week",
        ...project,
        created_at: now(),
      };
      db.projects.push(full);
      persist();
      return { ...full, task_total: 0, task_done: 0, goal_done_tasks: 0, goal_done_mins: 0 };
    },
    update_project: ({ id, patch }) => {
      const p = db.projects.find((p) => p.id === id);
      if (!p) throw `Проект не найден: ${id}`;
      for (const [k, v] of Object.entries(patch)) {
        if (v === undefined) continue;
        // конвенции бэкенда: пустая дата и цель <= 0 = снять
        if (k === "target_date") p.target_date = v === "" ? null : v;
        else if (k === "goal_tasks" || k === "goal_mins") p[k] = v > 0 ? v : null;
        else p[k] = v;
      }
      persist();
    },
    delete_project: ({ id }) => {
      db.projects = db.projects.filter((p) => p.id !== id);
      for (const t of db.tasks) if (t.project_id === id) t.project_id = null;
      persist();
    },
    get_goal_history: ({ projectId }) => [],

    // --- подзадачи ---
    get_subtasks: ({ taskId }) => findTask(taskId)?.subtasks ?? [],
    get_task_blockers: ({ taskId }) => openBlockers(taskId),
    add_task_dependency: ({ taskId, blockerId }) => {
      if (taskId === blockerId) throw "Задача не может блокировать саму себя";
      // Цикл ловим тем же обходом цепочки, что и Rust: иначе обе задачи
      // остались бы заблокированными навсегда.
      const chain = (from, to, seen = new Set()) => {
        if (seen.has(from)) return false;
        seen.add(from);
        return (db.taskDeps ?? []).filter((d) => d.task_id === from)
          .some((d) => d.blocker_id === to || chain(d.blocker_id, to, seen));
      };
      if (chain(blockerId, taskId)) {
        throw "Циклическая зависимость: эта задача уже блокирует выбранную";
      }
      db.taskDeps = db.taskDeps ?? [];
      if (!db.taskDeps.some((d) => d.task_id === taskId && d.blocker_id === blockerId)) {
        db.taskDeps.push({ task_id: taskId, blocker_id: blockerId });
      }
      persist();
    },
    remove_task_dependency: ({ taskId, blockerId }) => {
      db.taskDeps = (db.taskDeps ?? []).filter(
        (d) => !(d.task_id === taskId && d.blocker_id === blockerId),
      );
      persist();
    },
    add_subtask: ({ taskId, title }) => {
      const t = findTask(taskId);
      if (!t) throw `Задача не найдена: ${taskId}`;
      const sub = { id: uuid(), task_id: taskId, title, done: false, position: t.subtasks.length };
      t.subtasks.push(sub);
      persist();
      return { ...sub };
    },
    toggle_subtask: ({ id }) => {
      for (const t of db.tasks) {
        const s = t.subtasks.find((s) => s.id === id);
        if (!s) continue;
        s.done = !s.done;
        persist();
        // Зеркало бэкенда: отметил последнюю подзадачу — задача выполнена
        // (subtasks.rs::complete_if_all_subtasks_done). Ветку про повторы и
        // блокировки не дублируем — она уже внутри complete_task, вызываем его.
        // Зеркало бэкенда: у повтора чек-лист из ОДНОГО пункта не автозакрывает
        // задачу. Завершение повтора снимает все галочки, поэтому следующая
        // отметка снова была бы «последней» — и каждый круг двигал дедлайн и
        // сбрасывал notified_*, то есть заново ставил задачу в очередь уведомлений.
        const repeats = t.recurrence && t.recurrence !== "None";
        if (t.subtasks.length > 0 && t.subtasks.every((x) => x.done)
            && !(repeats && t.subtasks.length < 2)
            && !t.hidden && !t.deleted_at) {
          // Заблокированную задачу закрыть нельзя, но правка чек-листа при этом
          // должна уцелеть — в Rust это `let _ =`, здесь try/catch.
          try { commands.complete_task({ id: t.id }); } catch { /* остаётся открытой */ }
        }
        return;
      }
    },
    delete_subtask: ({ id }) => {
      for (const t of db.tasks) t.subtasks = t.subtasks.filter((s) => s.id !== id);
      persist();
    },
    rename_subtask: ({ id, title }) => {
      const trimmed = (title ?? "").trim();
      if (!trimmed) throw "Пустая подзадача";
      for (const t of db.tasks) {
        const s = t.subtasks.find((s) => s.id === id);
        if (s) { s.title = trimmed; persist(); return; }
      }
    },

    // --- категории задач ---
    get_categories: () => [...db.categories].sort((a, b) => a.position - b.position).map((c) => ({ ...c })),
    create_category: ({ name, color }) => {
      const trimmed = (name ?? "").trim();
      if (!trimmed) throw "Название категории не может быть пустым";
      const cat = {
        id: uuid(),
        name: trimmed,
        color: color || "#888888",
        position: Math.max(-1, ...db.categories.map((c) => c.position)) + 1,
      };
      db.categories.push(cat);
      persist();
      return { ...cat };
    },
    update_category: ({ id, patch }) => {
      const c = db.categories.find((c) => c.id === id);
      if (!c) throw `Категория не найдена: ${id}`;
      if (patch.name !== undefined && patch.name !== null) c.name = patch.name;
      if (patch.color !== undefined && patch.color !== null) c.color = patch.color;
      persist();
    },
    delete_category: ({ id }) => {
      if (id === "Other") throw "Категорию «Другое» нельзя удалить — это фолбэк";
      for (const t of db.tasks) if (t.category === id) t.category = "Other";
      db.categories = db.categories.filter((c) => c.id !== id);
      persist();
    },

    // --- статусы задач (v0.9.20, канбан) ---
    get_statuses: () => [...db.statuses].sort((a, b) => a.position - b.position).map((s) => ({ ...s })),
    create_status: ({ name, color }) => {
      const trimmed = (name ?? "").trim();
      if (!trimmed) throw "Название статуса не может быть пустым";
      const status = {
        id: uuid(),
        name: trimmed,
        color: color || "#888888",
        position: Math.max(-1, ...db.statuses.map((s) => s.position)) + 1,
        is_reserved: false,
      };
      db.statuses.push(status);
      persist();
      return { ...status };
    },
    update_status: ({ id, patch }) => {
      const s = db.statuses.find((s) => s.id === id);
      if (!s) throw `Статус не найден: ${id}`;
      if (s.is_reserved && patch.name !== undefined && patch.name !== null) {
        throw "Встроенный статус нельзя переименовать";
      }
      if (patch.name !== undefined && patch.name !== null) s.name = patch.name;
      if (patch.color !== undefined && patch.color !== null) s.color = patch.color;
      persist();
    },
    delete_status: ({ id }) => {
      const s = db.statuses.find((s) => s.id === id);
      if (s?.is_reserved) throw "Встроенный статус нельзя удалить";
      for (const t of db.tasks) if (t.status === id) t.status = "Todo";
      db.statuses = db.statuses.filter((s) => s.id !== id);
      persist();
    },

    // --- заметки ---
    get_notes: () => db.notes.filter((n) => !n.deleted_at).map((n) => ({ ...n })),
    create_note: ({ note }) => {
      const full = {
        id: uuid(),
        tags: [],
        linked_task_id: null,
        project_id: null,
        pinned: false,
        reminder_at: null,
        ...note,
        created_at: now(),
        updated_at: now(),
      };
      db.notes.push(full);
      persist();
      return { ...full };
    },
    update_note: ({ id, patch }) => {
      const n = db.notes.find((n) => n.id === id);
      if (!n) throw `Заметка не найдена: ${id}`;
      for (const [k, v] of Object.entries(patch)) {
        if (v !== undefined) n[k] = v;
      }
      n.updated_at = now();
      persist();
      return { ...n };
    },
    // Мягкое удаление (v0.9.76): строка остаётся, ревизии сохраняются —
    // насовсем убирает только purge_deleted_note.
    delete_note: ({ id }) => {
      const n = db.notes.find((x) => x.id === id);
      if (n) n.deleted_at = new Date().toISOString();
      persist();
    },
    get_deleted_notes: () =>
      db.notes.filter((n) => n.deleted_at)
        .sort((a, b) => new Date(b.deleted_at) - new Date(a.deleted_at))
        .map((n) => ({ ...n })),
    restore_note: ({ id }) => {
      const n = db.notes.find((x) => x.id === id);
      if (n) n.deleted_at = null;
      persist();
    },
    purge_deleted_note: ({ id }) => {
      db.notes = db.notes.filter((n) => n.id !== id);
      if (db.noteRevisions) db.noteRevisions = db.noteRevisions.filter((r) => r.note_id !== id);
      persist();
    },
    get_note_revisions: ({ noteId }) =>
      (db.noteRevisions ?? [])
        .filter((r) => r.note_id === noteId)
        .sort((a, b) => (a.created_at < b.created_at ? 1 : -1))
        .map((r) => ({ id: r.id, created_at: r.created_at, size: r.content.length })),
    get_note_revision_content: ({ revisionId }) => {
      const r = (db.noteRevisions ?? []).find((r) => r.id === revisionId);
      if (!r) throw "Ревизия не найдена";
      return r.content;
    },
    restore_note_revision: ({ revisionId }) => {
      const r = (db.noteRevisions ?? []).find((r) => r.id === revisionId);
      if (!r) throw "Ревизия не найдена";
      const n = db.notes.find((n) => n.id === r.note_id);
      if (!n) throw "Заметка не найдена";
      if (!db.noteRevisions) db.noteRevisions = [];
      db.noteRevisions.push({ id: uuid(), note_id: n.id, content: n.content, created_at: now() });
      n.content = r.content;
      n.updated_at = now();
      persist();
      return { ...n };
    },
    search_notes: ({ query }) => {
      const q = (query ?? "").trim().toLowerCase();
      if (!q) return [];
      return db.notes
        .filter((n) => !n.deleted_at)
        .filter((n) => n.title.toLowerCase().includes(q) || n.content.toLowerCase().includes(q))
        .map((n) => ({ ...n }));
    },
    search_notes_snippet: ({ query }) => {
      const q = (query ?? "").trim().toLowerCase();
      if (!q) return [];
      return db.notes
        .filter((n) => !n.deleted_at)
        .filter((n) => n.title.toLowerCase().includes(q) || n.content.toLowerCase().includes(q))
        .map((n) => ({
          item: { ...n },
          snippet: n.content.replace(new RegExp(`(${q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "gi"), "<mark>$1</mark>"),
        }));
    },
    search_tasks_snippet: ({ query }) => {
      const q = (query ?? "").trim().toLowerCase();
      if (!q) return [];
      return db.tasks
        .filter((t) => t.title.toLowerCase().includes(q) || (t.description ?? "").toLowerCase().includes(q))
        .map((t) => ({
          item: { ...t },
          snippet: (t.description ?? "").replace(new RegExp(`(${q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "gi"), "<mark>$1</mark>"),
        }));
    },
    get_images_dir: () => "/mock/app-data/images",
    // Экспорт/импорт заметок в .md (v0.7.14): реального диска в e2e нет —
    // db.mdFiles эмулирует папку как {filename: content}, ключ dir игнорируется
    // (единственная "папка" на тест).
    export_notes_md: () => {
      db.mdFiles = {};
      const used = {};
      for (const n of db.notes) {
        const base = (n.title || "Без названия").trim() || "Без названия";
        const key = base.toLowerCase();
        used[key] = (used[key] ?? 0) + 1;
        const filename = used[key] === 1 ? `${base}.md` : `${base}-${used[key]}.md`;
        db.mdFiles[filename] = n.content;
      }
      persist();
      return Object.keys(db.mdFiles).length;
    },
    import_notes_md: () => {
      const files = db.mdFiles ?? {};
      let count = 0;
      for (const [filename, content] of Object.entries(files)) {
        const title = filename.replace(/\.md$/, "");
        db.notes.push({
          id: uuid(), title, content, tags: [], linked_task_id: null, project_id: null,
          created_at: now(), updated_at: now(),
        });
        count++;
      }
      persist();
      return count;
    },
    export_note_html: ({ path, html }) => {
      db.exportedHtml = { path, html };
      persist();
    },
    save_note_image: ({ dataBase64, ext }) => {
      const filename = `${uuid()}.${ext}`;
      if (!db.images) db.images = [];
      db.images.push({ filename, dataUrl: dataBase64 });
      persist();
      return filename;
    },
    rename_note_links: ({ oldTitle, newTitle }) => {
      const oldT = (oldTitle ?? "").trim();
      const newT = (newTitle ?? "").trim();
      if (!oldT || oldT.toLowerCase() === newT.toLowerCase()) return 0;
      // Зеркало бэкенда: [[old]] / [[old|alias]] → [[new]] / [[new|alias]], без учёта регистра
      const re = /\[\[([^\[\]]+)\]\]/g;
      let count = 0;
      for (const n of db.notes) {
        let changed = false;
        n.content = n.content.replace(re, (m, inner) => {
          const pipeIdx = inner.indexOf("|");
          const target = pipeIdx >= 0 ? inner.slice(0, pipeIdx) : inner;
          const alias = pipeIdx >= 0 ? inner.slice(pipeIdx + 1) : null;
          if (target.trim().toLowerCase() !== oldT.toLowerCase()) return m;
          changed = true;
          return alias !== null ? `[[${newT}|${alias}]]` : `[[${newT}]]`;
        });
        if (changed) { n.updated_at = now(); count++; }
      }
      if (count > 0) persist();
      return count;
    },

    // --- дашборд / ИИ / модель ---
    get_activity_by_day: () => [],
    get_task_completions_by_day: () => {
      // Локальные сутки, как date(completed_at,'localtime') в бэкенде
      const byDay = {};
      for (const t of db.tasks) {
        if (!t.completed_at) continue;
        const k = localDayKey(t.completed_at);
        byDay[k] = (byDay[k] ?? 0) + 1;
      }
      return Object.entries(byDay).sort().map(([date, completed]) => ({ date, completed }));
    },
    get_completions_for_day: ({ date }) =>
      db.tasks.filter((t) => t.completed_at && localDayKey(t.completed_at) === date).map((t) => ({ id: t.id, title: t.title })),
    get_hourly_activity: () => [],
    // Помодоро: мок не гоняет реальный цикл (Study-режим не в скоупе e2e) —
    // фиксированное "off", тесты виджета переопределяют через db.pomodoro при сидировании.
    get_pomodoro_state: () => db.pomodoro ?? { phase: "off", until: null },
    pomodoro_toggle_pause: () => {
      if (!db.pomodoro) return;
      db.pomodoro.phase = db.pomodoro.phase === "paused" ? "work" : "paused";
      persist();
    },
    pomodoro_skip: () => {
      if (!db.pomodoro) return;
      db.pomodoro.phase = db.pomodoro.phase === "work" ? "break" : "work";
      persist();
    },
    pomodoro_start: () => {
      db.pomodoro = { phase: "work", until: new Date(Date.now() + 25 * 60000).toISOString() };
      persist();
    },
    pomodoro_stop: () => {
      db.pomodoro = { phase: "off", until: null };
      persist();
    },
    get_pomodoro_stats: () =>
      db.pomodoroStats ?? { today: 0, week: 0, task_streak: 0, pomodoro_streak: 0 },
    // Пончик «Выполнено по категориям»: пусто, если сид не задал иного
    // (db.categoryDistribution: [{category, count}]) — v0.9.47
    get_category_distribution: () => db.categoryDistribution ?? [],
    get_active_idle_ratio: () => ({ today_active: 0, today_idle: 0, week_active: 0, week_idle: 0 }),
    // v0.9.30: мониторинга в e2e нет, поэтому простой берётся из сида
    // (db.blockIdle: { "YYYY-MM-DD": [{task_id, idle_mins, ...}] }).
    get_block_idle: ({ date }) => (db.blockIdle ?? {})[date] ?? [],
    // v0.9.31: домены — из сида (db.domainUsage), пусто по умолчанию, как и
    // в реальности при выключенном track_domains.
    get_domain_usage: () => db.domainUsage ?? [],
    clear_domain_history: () => {
      const n = (db.domainUsage ?? []).length;
      db.domainUsage = [];
      persist();
      return n;
    },
    // v0.9.54: сид, а не пустой список. Раньше панель приложений не
    // рендерилась вообще ({#if appUsage.length > 0}), и переключатель периода
    // внутри неё был недосягаем для тестов.
    get_app_usage: () => db.appUsage ?? [],
    // Зеркало uncategorized_apps_impl: отдаём только приложения без правила.
    // Без этого фильтра тест не отличил бы «ИИ предложил» от «показали всё».
    get_uncategorized_apps: () => {
      const rules = JSON.parse(db.settings?.app_category_rules || "[]");
      const covered = (app) => rules.some((r) => {
        const p = String(r.pattern).toLowerCase();
        const t = String(app).toLowerCase();
        if (!p.includes("*")) return p === t;
        return new RegExp("^" + p.split("*").map((x) => x.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$").test(t);
      });
      return (db.appUsage ?? []).filter((a) => !covered(a.app));
    },
    // Ответ модели задаёт тест через db.aiAppRules — как и у остальных ИИ-команд,
    // чтобы проверять разбор и подтверждение, а не саму модель.
    ai_suggest_app_rules: () => {
      const rules = db.aiAppRules ?? [];
      setTimeout(() => window.__mockEmit("ai-app-rules", { rules, error: db.aiAppRulesError ?? null }), 0);
    },
    get_app_category_time: () => [],
    dashboard_insight: () => {},
    summarize_day: () => {},
    summarize_week: () => {},
    // Планировщик: детерминированный «ИИ» — первый бэклог-таск в 10:00 на 60 мин
    ai_plan_day: () => {
      const t = db.tasks.find(
        (t) => !t.hidden && !t.scheduled_at && (t.status === "Todo" || t.status === "InProgress"),
      );
      const at = new Date();
      at.setHours(10, 0, 0, 0);
      setTimeout(() => window.__mockEmit("ai-plan", {
        blocks: t ? [{ id: t.id, title: t.title, scheduled_at: at.toISOString(), mins: 60 }] : [],
        error: t ? null : "Бэклог пуст — нечего планировать",
      }), 0);
    },
    ai_what_now: () => {
      setTimeout(() => window.__mockEmit("ai-what-now", {
        result: "Совет мока: начните с самой приоритетной задачи.",
        error: null,
      }), 0);
    },
    ai_rewrite: () => {},
    ai_subtasks: () => {},
    ai_classify: () => {},
    // Автолинковка: детерминированный «ИИ» — предлагает все остальные заметки (до 5)
    ai_suggest_links: ({ noteId }) => {
      const titles = db.notes.filter((n) => n.id !== noteId).map((n) => n.title).slice(0, 5);
      setTimeout(() => window.__mockEmit("ai-links", {
        note_id: noteId,
        titles,
        error: titles.length === 0 ? "Больше нет заметок, с которыми можно связать эту" : null,
      }), 0);
    },
    // ИИ по выделению (v0.9.09): детерминированный «ИИ» — просто помечает
    // текст меткой действия, чтобы e2e-тест мог проверить, что в редактор
    // подставился именно результат нужного запроса.
    ai_edit_selection: ({ requestId, text, mode }) => {
      setTimeout(() => window.__mockEmit("ai-selection-result", {
        request_id: requestId,
        result: `[${mode}] ${text}`,
        error: null,
      }), 0);
    },
    // ИИ: резюме заметки (v0.9.10) — детерминированный «ИИ» возвращает
    // фиксированный список пунктов с длиной исходного текста внутри, чтобы
    // тест мог проверить и сам факт вызова, и то, что окно показывает
    // именно результат этого запроса.
    ai_summarize_note: ({ requestId, text }) => {
      setTimeout(() => window.__mockEmit("ai-note-summary", {
        request_id: requestId,
        result: `- Пункт резюме (длина текста: ${text.length})`,
        error: null,
      }), 0);
    },
    // ИИ: извлечение задач из заметки (v0.9.11) — детерминированный «ИИ»
    // возвращает фиксированный список из 2 задач, чтобы e2e-тест мог
    // проверить и предпросмотр, и последующее создание задач по клику.
    ai_extract_tasks: ({ requestId }) => {
      setTimeout(() => window.__mockEmit("ai-extract-tasks", {
        request_id: requestId,
        items: ["Купить билеты", "Забронировать отель"],
        error: null,
      }), 0);
    },
    // Спросить заметки (v0.10.20). Мок повторяет ГЛАВНОЕ свойство бэкенда:
    // ретривал идёт через тот же поиск, и когда он пуст — ответ «не нашлось»
    // отдаётся без обращения к модели. Именно это защищает от выдуманного
    // ответа, поэтому именно это и должен проверять e2e.
    ai_ask_notes: ({ requestId, question }) => {
      const q = (question ?? "").trim().toLowerCase();
      const found = db.notes
        .filter((n) => !n.deleted_at)
        .filter((n) => n.title.toLowerCase().includes(q) || n.content.toLowerCase().includes(q))
        .slice(0, 5);
      setTimeout(() => window.__mockEmit("ai-ask-notes", {
        request_id: requestId,
        result: found.length
          ? `Ответ по заметкам: ${found.map((n) => n.title).join(", ")}`
          : "В заметках ничего не нашлось по этому вопросу. Поиск идёт по словам, поэтому попробуйте другую формулировку.",
        sources: found.map((n) => ({ id: n.id, title: n.title })),
        error: null,
      }), 0);
    },
    model_status: ({ kind }) => db.models?.[kind ?? "llm"] ?? { exists: false, size_bytes: 0 },
    // v0.9.28: путь приходит от бэкенда (app_data_dir зависит от ОС), в моке
    // отдаём линуксовый вид — тест проверяет, что показан ответ команды, а не
    // зашитая в UI строка.
    // v0.9.64: путь зависит от типа модели.
    model_path: ({ kind }) =>
      kind === "whisper"
        ? "/home/user/.local/share/com.kriptag.app/models/whisper.bin"
        : "/home/user/.local/share/com.kriptag.app/models/model.gguf",
    // v0.9.64: каталог зависит от kind, и мок различает его нарочно — иначе тест
    // не заметил бы, что фронтенд перестал передавать тип и оба загрузчика
    // показывают один и тот же список.
    list_model_options: ({ kind }) =>
      kind === "whisper"
        ? [
            // Описания — дословно из model.rs: тест на полноту перевода читает
            // именно то, что видно на экране, а сокращённая выдумка прошла бы мимо
            // словаря и «нашла» кириллицу, которой в приложении нет.
            { id: "whisper-tiny", name: "Whisper Tiny", url: "https://example.com/ggml-tiny.bin", size_bytes: 77700000, description: "Самая лёгкая и быстрая — распознаёт почти мгновенно даже на слабой машине, но заметно путает слова, особенно в русской речи.", ram_gb: 1, recommended: false, kind: "whisper" },
            { id: "whisper-base", name: "Whisper Base", url: "https://example.com/ggml-base.bin", size_bytes: 148000000, description: "Разумный баланс — держит русскую речь заметно лучше Tiny и всё ещё быстрая на обычном процессоре.", ram_gb: 1, recommended: true, kind: "whisper" },
          ]
        : [
            { id: "qwen2.5-0.5b", name: "Qwen2.5 0.5B Instruct", url: "https://example.com/qwen2.5-0.5b.gguf", size_bytes: 491000000, description: "Самая быстрая и лёгкая — базовое качество.", ram_gb: 2, recommended: false, kind: "llm" },
            { id: "qwen2.5-1.5b", name: "Qwen2.5 1.5B Instruct", url: "https://example.com/qwen2.5-1.5b.gguf", size_bytes: 1120000000, description: "Баланс скорости и качества.", ram_gb: 3, recommended: true, kind: "llm" },
            { id: "phi-3.5-mini", name: "Phi-3.5 Mini Instruct", url: "https://example.com/phi-3.5-mini.gguf", size_bytes: 2390000000, description: "Лучшее качество, но медленнее.", ram_gb: 5, recommended: false, kind: "llm" },
          ],
    // Голосовой ввод (v0.9.65). Доступность управляется сидом: по умолчанию
    // выключено — так же, как у настоящего пользователя без скачанной модели,
    // поэтому существующие тесты не видят новой кнопки.
    voice_available: () => db.voiceAvailable === true,
    start_voice_recording: () => {
      if (db.voiceAvailable !== true) throw new Error("Голосовой ввод недоступен");
      db.voiceRecording = true;
      persist();
    },
    stop_voice_recording: () => {
      db.voiceRecording = false;
      persist();
      // Реального микрофона в e2e нет: мок отдаёт заранее заданный текст, как
      // Rust отдал бы распознанный.
      return db.voiceText ?? "надиктованный текст";
    },
    cancel_voice_recording: () => {
      db.voiceRecording = false;
      persist();
    },
    download_model: ({ kind }) => {
      db.models = { ...(db.models ?? {}), [kind ?? "llm"]: { exists: true, size_bytes: 1024 } };
      persist();
    },
    export: () => {},
    import: () => {},
    // Значения берутся из db.importPreview, чтобы тест мог задать любой сценарий
    // (в т.ч. «в базе есть записи новее копии»). По умолчанию терять нечего.
    preview_import: () => db.importPreview ?? {
      tasks: 3, notes: 2, current_tasks: 3, current_notes: 2,
      newest: "2026-07-17T16:00:00+00:00",
      losing_tasks: 0, losing_notes: 0,
    },
    do_auto_backup: () => "kriptag-backup-2026-07-17-1600.zip",

    // --- трекинг ---
    start_task_tracking: ({ taskId }) => {
      const t = findTask(taskId);
      if (!t) throw `Задача не найдена: ${taskId}`;
      if (!db.sessions) db.sessions = [];
      // Закрыть открытые
      for (const s of db.sessions) if (!s.ended_at) s.ended_at = now();
      const s = { id: uuid(), task_id: taskId, started_at: now(), ended_at: null };
      db.sessions.push(s);
      t.status = "InProgress";
      t.updated_at = now();
      persist();
      return { task_id: taskId, title: t.title, started_at: s.started_at, elapsed_secs: 0 };
    },
    stop_task_tracking: () => {
      if (!db.sessions) return;
      for (const s of db.sessions) if (!s.ended_at) s.ended_at = now();
      persist();
    },
    get_active_session: () => {
      if (!db.sessions) return null;
      const s = db.sessions.find((s) => !s.ended_at);
      if (!s) return null;
      const t = findTask(s.task_id);
      const started = new Date(s.started_at);
      const elapsed = Math.round((Date.now() - started.getTime()) / 1000);
      return { task_id: s.task_id, title: t?.title ?? "", started_at: s.started_at, elapsed_secs: elapsed };
    },
    get_task_seconds: ({ taskId }) => {
      if (!db.sessions) return 0;
      const now = Date.now();
      let total = 0;
      for (const s of db.sessions) {
        if (s.task_id !== taskId) continue;
        const start = new Date(s.started_at).getTime();
        const end = s.ended_at ? new Date(s.ended_at).getTime() : now;
        total += Math.max(0, Math.round((end - start) / 1000));
      }
      return total;
    },
    get_project_seconds: ({ projectId, from }) => {
      if (!db.sessions) return 0;
      const fromMs = new Date(from).getTime();
      const nowMs = Date.now();
      let total = 0;
      for (const s of db.sessions) {
        const t = findTask(s.task_id);
        if (!t || t.project_id !== projectId) continue;
        const start = new Date(s.started_at).getTime();
        if (start < fromMs) continue;
        const end = s.ended_at ? new Date(s.ended_at).getTime() : nowMs;
        total += Math.max(0, Math.round((end - start) / 1000));
      }
      return total;
    },

    // --- рутины ---
    get_routines: () => (db.routines ?? []).map((r) => ({ ...r })),
    create_routine: ({ routine }) => {
      const full = {
        id: uuid(),
        active: true,
        ...routine,
      };
      if (!db.routines) db.routines = [];
      db.routines.push(full);
      persist();
      return { ...full };
    },
    update_routine: ({ id, patch }) => {
      const r = db.routines?.find((r) => r.id === id);
      if (!r) throw `Рутина не найдена: ${id}`;
      for (const [k, v] of Object.entries(patch)) {
        if (v !== undefined) r[k] = v;
      }
      persist();
    },
    delete_routine: ({ id }) => {
      if (db.routines) db.routines = db.routines.filter((r) => r.id !== id);
      persist();
    },

    // --- шаблоны чеклистов ---
    get_checklist_templates: () =>
      [...(db.checklistTemplates ?? [])].sort((a, b) => a.name.localeCompare(b.name)).map((t) => ({ ...t })),
    create_checklist_template: ({ name, items }) => {
      const cleanName = (name ?? "").trim();
      const cleanItems = (items ?? []).map((i) => i.trim()).filter(Boolean);
      if (!cleanName) throw "Название шаблона не может быть пустым";
      if (cleanItems.length === 0) throw "Шаблон без пунктов не имеет смысла";
      const full = { id: uuid(), name: cleanName, items: cleanItems };
      if (!db.checklistTemplates) db.checklistTemplates = [];
      db.checklistTemplates.push(full);
      persist();
      return { ...full };
    },
    delete_checklist_template: ({ id }) => {
      if (db.checklistTemplates) db.checklistTemplates = db.checklistTemplates.filter((t) => t.id !== id);
      persist();
    },

    // --- умные списки ---
    get_smart_lists: () =>
      [...(db.smartLists ?? [])].sort((a, b) => a.position - b.position || a.name.localeCompare(b.name)).map((l) => ({ ...l })),
    create_smart_list: ({ name, filter }) => {
      const cleanName = (name ?? "").trim();
      if (!cleanName) throw "Название списка не может быть пустым";
      const f = filter ?? {};
      const isEmpty = !f.category && !f.priority && !f.tag && f.has_deadline == null;
      if (isEmpty) throw "Список без условий фильтра не имеет смысла";
      if (!db.smartLists) db.smartLists = [];
      const position = db.smartLists.length;
      const full = { id: uuid(), name: cleanName, filter: f, position };
      db.smartLists.push(full);
      persist();
      return { ...full };
    },
    delete_smart_list: ({ id }) => {
      if (db.smartLists) db.smartLists = db.smartLists.filter((l) => l.id !== id);
      persist();
    },

    // --- центр уведомлений ---
    get_notification_log: () =>
      [...(db.notificationLog ?? [])].sort((a, b) => b.created_at.localeCompare(a.created_at)).slice(0, 100).map((n) => ({ ...n })),
    get_unread_notification_count: () =>
      (db.notificationLog ?? []).filter((n) => !n.read_at).length,
    mark_notifications_read: () => {
      const t = now();
      db.notificationLog = (db.notificationLog ?? []).map((n) => (n.read_at ? n : { ...n, read_at: t }));
      persist();
    },
    clear_notification_log: () => {
      db.notificationLog = [];
      persist();
    },

    // --- плагины ---
    "plugin:event|listen": ({ event, handler }) => {
      if (!eventHandlers.has(event)) eventHandlers.set(event, new Set());
      eventHandlers.get(event).add(handler);
      return handler;
    },
    "plugin:event|unlisten": ({ event, eventId }) => {
      eventHandlers.get(event)?.delete(eventId);
    },
    // Фронтенд тоже шлёт события, а не только слушает: QuickCapture объявляет
    // "task-created"/"note-created"/"task-updated" главному окну. Шим добавлен в
    // v0.9.80 — до него emit проваливался в undefined, событие не доходило ни до
    // одного слушателя, и в e2e эта связь не проверялась вовсе. Доставку делает
    // тот же __mockEmit, что и события «от бэкенда».
    "plugin:event|emit": ({ event, payload }) => {
      window.__mockEmit(event, payload);
    },
    "plugin:autostart|enable": () => {},
    "plugin:autostart|disable": () => {},
    "plugin:autostart|is_enabled": () => false,
    "plugin:dialog|save": () => db.mockDialogPath ?? null,
    "plugin:dialog|open": () => db.mockDialogPath ?? null,
    // confirm() плагина реализован поверх команды message и возвращает подпись
    // нажатой кнопки. Текст запоминается — тесты про импорт проверяют именно его,
    // а браузерный window.confirm тут не срабатывает (v0.9.92).
    "plugin:dialog|message": ({ message, buttons }) => {
      db.lastConfirm = message;
      persist();
      const ok = buttons?.ok ?? "Ok";
      return db.confirmAnswer === false ? "Cancel" : ok;
    },
    // Своя титульная строка (v0.9.40): WindowControls вызывает isMaximized() при
    // монтировании и на каждый resize. Шим добавлен в v0.9.80 — до него вызов
    // проваливался в undefined, .catch() его глотал, и состояние кнопки
    // «развернуть» не проверялось ни одним тестом. Нашёл это новый страж
    // __unknownInvokes, а не человек.
    // Копирование в буфер (резюме ИИ, Notes.svelte:528). Шим добавлен в v0.9.80:
    // до него тест «клик копирует и закрывает» проверял только половину про
    // закрытие — сам вызов копирования проваливался в undefined. Записанное
    // складываем в db, чтобы тест мог проверить именно текст.
    "plugin:clipboard-manager|write_text": ({ text }) => {
      db.clipboardWritten = text;
      persist();
    },
    "plugin:window|is_maximized": () => db.windowMaximized ?? false,
    "plugin:window|toggle_maximize": () => {
      db.windowMaximized = !(db.windowMaximized ?? false);
      persist();
    },
    "plugin:window|minimize": () => {},
    "plugin:window|hide": () => {},
  };

  window.__TAURI_INTERNALS__ = {
    transformCallback(cb) {
      const id = nextCallbackId++;
      callbacks.set(id, cb);
      return id;
    },
    async invoke(cmd, args = {}) {
      // Инъекция сбоя (v0.9.25): тест ставит window.__mockFailNext = {cmd, msg}
      // и следующий вызов этой команды падает, как упала бы Rust-команда.
      // Нужно, чтобы проверить показ ошибки в UI: обычные пути мока не падают,
      // а без реального сбоя баннер ошибки нечем вызвать.
      const fail = window.__mockFailNext;
      if (fail && fail.cmd === cmd) {
        window.__mockFailNext = null;
        throw fail.msg;
      }
      const handler = commands[cmd];
      if (!handler) {
        window.__unknownInvokes.push(cmd);
        return undefined;
      }
      return handler(args);
    },
    // Картинки в заметках (v0.7.13): реальный convertFileSrc строит asset://
    // URL из абсолютного пути — в e2e картинок на диске нет, поэтому отдаём
    // содержимое из db.images (заполняется save_note_image-моком) как data-url.
    convertFileSrc(filePath) {
      const name = filePath.split("/").pop();
      const entry = (db.images ?? []).find((i) => i.filename === name);
      return entry ? entry.dataUrl : `mock-asset://${filePath}`;
    },
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
  };
})();
