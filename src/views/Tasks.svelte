<script lang="ts">
  import { onMount, tick } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { taskStore } from "../lib/stores/tasks.svelte";
  import { projectStore } from "../lib/stores/projects.svelte";
  import { categoryStore } from "../lib/stores/categories.svelte";
  import { statusStore } from "../lib/stores/statuses.svelte";
  import { smartListStore } from "../lib/stores/smartLists.svelte";
  import { pinnedStore } from "../lib/stores/pinned.svelte";
  import { api } from "../lib/api/tauri";
  import { parseComposer, parseTaskText, matchCategoryQuery, SUBTASK_PREFIX } from "../lib/composer";
  import { t, tErr } from "../lib/i18n.svelte";
  import TaskModal from "../lib/components/TaskModal.svelte";
  import ChecklistEditor from "../lib/components/ChecklistEditor.svelte";
  import { parseChecklist, formatChecklist } from "../lib/checklistText";
  import TaskHistoryDetail from "../lib/components/TaskHistoryDetail.svelte";
  import Icon from "../lib/components/Icon.svelte";
  import ContextMenu from "../lib/components/ContextMenu.svelte";
  import type { Task, Subtask, Category, CreateTaskPayload, UpdateTaskPayload, Project, GoalSnapshot, ActiveSession, SmartListFilter } from "../lib/types";
  import { hhmm } from "../lib/datetime";
  import { isTypingTarget, actionForKey, nextIndex, reconcileIndex } from "../lib/listnav";
  import { unblockCounts } from "../lib/blockers";
  import { onAccentText } from "../lib/surfaces";
  import { loadUiState, saveUiState, restoreOneOf, restoreValid, restoreNumber, restoreNumberMap } from "../lib/uistate";

  type AiResult = { task_id: string; type: string; result?: string; error?: string };

  let showGoalHistory = $state<Record<string, GoalSnapshot[]>>({});
  let goalHistoryLoading = $state<Record<string, boolean>>({});

  // List/History/Trash is a single mutually exclusive switch. These used to be two
  // independent toggles, so both could be open at once as two nearly
  // indistinguishable blocks under the shared list.
  let listSubView = $state<"active" | "history" | "trash">("active");
  let showCreateModal = $state(false);
  let editingTask: Task | null = $state(null);

  /* How long a single click waits to see whether it is really the first half of a
     double click. The platform's own threshold is not exposed to the web, and this
     matches what other toolkits use; shorter and a slow double click opens the
     editor first, longer and renaming feels laggy. */
  const DBLCLICK_MS = 250;
  let historyDetailTask: Task | null = $state(null);

  // List/Board is a switch in the page head. This used to be a separate
  // Kanban.svelte page and was merged here so the project filter, smart lists and
  // multi-select are shared by both view modes.
  // List/Board survives a restart (v0.9.79); the History/Trash sub-view above does
  // not, on purpose — opening straight into the Trash is disorienting.
  let viewMode = $state<"list" | "board">(restoreOneOf(loadUiState().taskViewMode, ["list", "board"] as const, "list"));

  // Board column widths, dragged by the divider to the column's right (v0.10.06).
  // The bounds are the column's own limits, not taste: below MIN a card's chips
  // wrap onto their own lines and the card stops being scannable, and past MAX a
  // single column crowds the others off a 1600px board.
  //
  // Stored per status id, not as one shared number: the columns are independent,
  // and a status can be added or deleted between launches. A column with no entry
  // simply gets COL_DEFAULT, so a new status needs no migration.
  const COL_MIN = 180, COL_MAX = 520, COL_DEFAULT = 260;
  let colWidths = $state<Record<string, number>>(
    restoreNumberMap(loadUiState().boardColWidths, COL_MIN, COL_MAX),
  );
  let resizing = $state(false);

  // The columns on screen, in order. Derived once rather than filtered at each
  // use: the handles index into this list to find the column they resize, so a
  // second copy filtered differently would move the wrong column.
  const boardColumns = $derived(statusStore.statuses.filter(st => st.id !== "Archived"));

  const widthOf = (statusId: string) => colWidths[statusId] ?? COL_DEFAULT;

  function setColWidth(statusId: string, px: number) {
    colWidths = { ...colWidths, [statusId]: Math.min(COL_MAX, Math.max(COL_MIN, px)) };
  }

  function startColResize(e: PointerEvent, statusId: string) {
    e.preventDefault();
    resizing = true;
    const startX = e.clientX;
    const startW = widthOf(statusId);
    const el = e.currentTarget as HTMLElement;
    // Pointer capture, so the drag survives the cursor leaving the handle —
    // without it a quick pull drops the grab as soon as it outruns the divider.
    el.setPointerCapture(e.pointerId);

    const move = (ev: PointerEvent) => setColWidth(statusId, startW + (ev.clientX - startX));
    const up = () => {
      resizing = false;
      el.releasePointerCapture(e.pointerId);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      saveUiState({ boardColWidths: colWidths });
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  }
  $effect(() => { saveUiState({ taskViewMode: viewMode }); });

  // Projects: the list filter ("all" | "none" | id) and the management modal
  let projectFilter = $state<string>("all");

  // Categories: "all" or a category id (v0.9.99). A row of coloured chips rather
  // than a <select> like the project filter — the category's own colour is the
  // thing being picked, and a native option cannot carry it.
  let categoryFilter = $state<string>("all");
  let showProjects = $state(false);
  let newProjectName = $state("");

  // Smart lists: the modal for creating one of your own
  let showSmartListModal = $state(false);
  let newSmartListName = $state("");
  let newSmartListCategory = $state("");
  let newSmartListPriority = $state("");
  let newSmartListTag = $state("");
  let newSmartListHasDeadline = $state<"" | "yes" | "no">("");

  function resetSmartListForm() {
    newSmartListName = "";
    newSmartListCategory = "";
    newSmartListPriority = "";
    newSmartListTag = "";
    newSmartListHasDeadline = "";
  }

  async function createSmartList() {
    const filter: SmartListFilter = {
      category: newSmartListCategory || null,
      priority: newSmartListPriority || null,
      tag: newSmartListTag.trim() || null,
      has_deadline: newSmartListHasDeadline === "" ? null : newSmartListHasDeadline === "yes",
    };
    await smartListStore.create(newSmartListName, filter);
    if (!smartListStore.error) {
      showSmartListModal = false;
      resetSmartListForm();
    }
  }

  async function removeSmartList(id: string) {
    if (activeSmartListId === id) activeSmartListId = null;
    await smartListStore.remove(id);
  }

  onMount(() => {
    // The saved filters are restored after these two have loaded, not on a guess
    // about the stores being non-empty: a user with no projects and no custom smart
    // lists would otherwise never reach the restore at all.
    Promise.all([projectStore.load(), smartListStore.load()]).then(restoreSavedFilters);
    categoryStore.load();
    statusStore.load();
    pinnedStore.load();
    // Capability detection: with AI turned off the "What now?" button is simply hidden
    api.getSettings().then(s => {
      aiEnabled = s.ai_provider !== "none";
      autoExpandSubs = s.show_subtasks_expanded;
    }).catch(() => {});
  });

  let aiEnabled = $state(false);
  // Tasks with subtasks are expanded by default (the "Appearance" setting)
  let autoExpandSubs = $state(true);

  // Smart lists: the built-in ones ("Overdue"/"This week") depend on the current
  // date, so they live entirely on the frontend and are not stored in the DB;
  // user-defined ones come from smartListStore, with a predicate over category,
  // priority, tag and whether a deadline is set.
  type BuiltinSmartList = { id: string; name: string; test: (t: Task) => boolean };
  const BUILTIN_SMART_LISTS: BuiltinSmartList[] = $derived([
    {
      id: "__overdue",
      name: t("Просроченные"),
      test: (t) => !!t.deadline && new Date(t.deadline).getTime() < Date.now(),
    },
    {
      id: "__this_week",
      name: t("На этой неделе"),
      test: (t) => {
        if (!t.deadline) return false;
        const d = new Date(t.deadline).getTime();
        const now = Date.now();
        return d >= now && d <= now + 7 * 864e5;
      },
    },
    {
      // No date involved, unlike the two above, but built-in for the same reason:
      // blocked_by is computed per request rather than stored, so a saved
      // SmartListFilter (category/priority/tag/deadline) cannot express it.
      id: "__blocked",
      name: t("Заблокированные"),
      test: (t) => t.blocked_by.length > 0,
    },
  ]);

  let activeSmartListId: string | null = $state(null);

  // The smart list and the project filter are restored only once their sources have
  // loaded: a list or project deleted between two launches must not survive as a
  // filter that matches nothing — the user cannot tell that from "no tasks here".
  // A single run (restoredFilters) rather than an $effect, or deleting the active
  // smart list during a session would immediately reset the filter under the user.
  // Called from onMount once the projects and smart lists have arrived: validating
  // a saved id against empty stores would discard it as "deleted".
  let restoredFilters = $state(false);
  function restoreSavedFilters() {
    const saved = loadUiState();
    const smartIds = [...BUILTIN_SMART_LISTS.map(l => l.id), ...smartListStore.lists.map(l => l.id)];
    activeSmartListId = restoreValid(saved.smartListId, (id) => smartIds.includes(id), null);
    projectFilter = restoreValid(
      saved.projectFilter,
      (id) => id === "all" || id === "none" || projectStore.projects.some(p => p.id === id),
      "all",
    );
    // A category can be deleted between two launches. restoreValid drops the
    // saved id silently in that case — restoring it blindly would open a list
    // filtered by something that no longer exists, which reads as "I have no
    // tasks" with no way to tell the difference.
    categoryFilter = restoreValid(
      saved.categoryFilter,
      (id) => id === "all" || categoryStore.categories.some(c => c.id === id),
      "all",
    );
    restoredFilters = true;
  }

  // Saving starts only after the restore, otherwise the initial defaults would
  // overwrite the very values still waiting to be read back.
  $effect(() => {
    if (!restoredFilters) return;
    saveUiState({ smartListId: activeSmartListId, projectFilter, categoryFilter });
  });

  // "Unblocks N" — the reverse of blocked_by (v0.9.78). Counted over activeTasks
  // rather than over the visible list on purpose: filtering by project or by a
  // smart list must not shrink the number. The badge answers "how many tasks does
  // finishing this free up", and that is a property of the task, not of the
  // current filter — a count that changed with the filter would be misleading.
  const unblocks = $derived(unblockCounts(taskStore.activeTasks));

  function matchesSmartFilter(t: Task, f: SmartListFilter): boolean {
    if (f.category && t.category !== f.category) return false;
    if (f.priority && t.priority !== f.priority) return false;
    if (f.tag && !t.tags.includes(f.tag)) return false;
    if (f.has_deadline === true && !t.deadline) return false;
    if (f.has_deadline === false && t.deadline) return false;
    return true;
  }

  const activeSmartListTest = $derived.by((): ((t: Task) => boolean) | null => {
    if (!activeSmartListId) return null;
    const builtin = BUILTIN_SMART_LISTS.find(l => l.id === activeSmartListId);
    if (builtin) return builtin.test;
    const custom = smartListStore.lists.find(l => l.id === activeSmartListId);
    if (custom) return (t: Task) => matchesSmartFilter(t, custom.filter);
    return null;
  });

  const filteredActive = $derived(
    taskStore.activeTasks
      .filter(t =>
        projectFilter === "all" ? true :
        projectFilter === "none" ? !t.project_id :
        t.project_id === projectFilter
      )
      .filter(t => categoryFilter === "all" ? true : t.category === categoryFilter)
      .filter(t => activeSmartListTest ? activeSmartListTest(t) : true)
  );

  // The board uses the same project, category and smart-list filters as the list, but over
  // taskStore.tasks rather than activeTasks: completed tasks (hidden=true, the same
  // flag that moves them into History in list mode) must stay visible in their own
  // column rather than vanishing from the whole board.
  const boardTasks = $derived(
    taskStore.tasks
      .filter(t => t.status !== "Archived")
      .filter(t =>
        projectFilter === "all" ? true :
        projectFilter === "none" ? !t.project_id :
        t.project_id === projectFilter
      )
      .filter(t => categoryFilter === "all" ? true : t.category === categoryFilter)
      .filter(t => activeSmartListTest ? activeSmartListTest(t) : true)
  );

  // The multi-selection does not survive a change of the visible list (filter,
  // search, switching smart lists), otherwise a bulk action could quietly affect
  // rows that are no longer on screen.
  $effect(() => {
    const visible = new Set(filteredActive.map(t => t.id));
    if ([...selectedIds].some(id => !visible.has(id))) {
      selectedIds = new Set([...selectedIds].filter(id => visible.has(id)));
    }
  });

  // Grouping by "all projects": one section per project (in the projects' own
  // order) plus "No project".
  const grouped = $derived.by(() => {
    if (projectFilter !== "all" || projectStore.projects.length === 0) return null;
    const groups: { id: string; name: string; done: number; total: number; tasks: Task[]; project: Project | null }[] = [];
    for (const p of projectStore.projects) {
      const tasks = filteredActive.filter(t => t.project_id === p.id);
      if (tasks.length > 0) {
        groups.push({ id: p.id, name: p.name, done: p.task_done, total: p.task_total, tasks, project: p });
      }
    }
    const orphan = filteredActive.filter(t => !t.project_id || !projectStore.projects.some(p => p.id === t.project_id));
    if (orphan.length > 0 && groups.length > 0) {
      groups.push({ id: "", name: t("Без проекта"), done: 0, total: 0, tasks: orphan, project: null });
    }
    return groups.length > 0 ? groups : null;
  });

  // A project's goal: the progress text "done/target tasks · done/target min" and its status
  function goalText(p: Project): string | null {
    if (p.goal_tasks == null && p.goal_mins == null) return null;
    const parts: string[] = [];
    if (p.goal_tasks != null) parts.push(t("{done}/{total} задач", { done: p.goal_done_tasks, total: p.goal_tasks }));
    if (p.goal_mins != null) parts.push(t("{done}/{total} мин", { done: p.goal_done_mins, total: p.goal_mins }));
    return parts.join(" · ");
  }

  function goalMet(p: Project): boolean {
    return (p.goal_tasks == null || p.goal_done_tasks >= p.goal_tasks)
        && (p.goal_mins == null || p.goal_done_mins >= p.goal_mins);
  }

  async function toggleGoalHistory(projectId: string) {
    if (showGoalHistory[projectId]) {
      const next = { ...showGoalHistory };
      delete next[projectId];
      showGoalHistory = next;
      return;
    }
    goalHistoryLoading = { ...goalHistoryLoading, [projectId]: true };
    try {
      const snapshots = await api.getGoalHistory(projectId);
      showGoalHistory = { ...showGoalHistory, [projectId]: snapshots };
    } finally {
      goalHistoryLoading = { ...goalHistoryLoading, [projectId]: false };
    }
  }

  async function addProject() {
    const name = newProjectName.trim();
    if (!name) return;
    await projectStore.create(name);
    newProjectName = "";
  }

  // The day's schedule: today's time blocks (assigned in Calendar -> Week)
  const todayBlocks = $derived.by(() => {
    const today = new Date().toDateString();
    return taskStore.activeTasks
      .filter(t => t.scheduled_at && new Date(t.scheduled_at).toDateString() === today)
      .sort((a, b) => a.scheduled_at!.localeCompare(b.scheduled_at!));
  });

  function blockTime(t: Task): string {
    const start = new Date(t.scheduled_at!);
    const end = new Date(start.getTime() + (t.scheduled_mins ?? 60) * 60_000);
    return `${hhmm(start)}–${hhmm(end)}`;
  }

  let searchQuery = $state("");
  let searchResults = $state<Task[]>([]);
  let isSearching = $state(false);

  let aiLoadingId: string | null = $state(null);
  let aiError: string | null = $state(null);
  let subtasksPreview: { taskId: string; items: string[] } | null = $state(null);

  let trackingId: string | null = $state(null);

  onMount(() => {
    api.getActiveSession().then(s => { trackingId = s?.task_id ?? null; }).catch(() => {});
  });

  // Completing via the row's ✓. Tracking must be stopped explicitly here: this path
  // bypasses moveToStatus (which stops it the same way when leaving InProgress),
  // and without that the timer kept ticking on an already-completed task.
  async function completeRow(task: Task) {
    if (trackingId === task.id) {
      await api.stopTaskTracking();
      trackingId = null;
    }
    // An unsaved checklist edit is flushed BEFORE completing: the line below drops
    // the panel's cache, and without the flush the edit would go with it. Verified:
    // renaming while completing "immediately" lost not the text but the subtask
    // itself — an empty list was left in the DB.
    await flushSubs(task);
    await taskStore.complete(task.id);
    // The panel's cache must go: it lives separately from the store, so after the
    // checklist is reset (a recurring task moving to its next run) the screen would
    // keep ticks that no longer exist in the DB. It also fixes a race — a deferred
    // write arriving after the reset finds no text and does not restore the ticks.
    delete subsText[task.id];
    projectStore.load();
  }

  async function toggleTracking(taskId: string) {
    if (trackingId === taskId) {
      await api.stopTaskTracking();
      trackingId = null;
    } else {
      await api.startTaskTracking(taskId);
      trackingId = taskId;
    }
    taskStore.load();
  }

  // --- The board: one column per status from statusStore rather than a hardcoded
  // Todo/InProgress/Done, since the user can add their own. ---
  function boardTasksFor(statusId: string): Task[] {
    return boardTasks
      .filter(t => t.status === statusId)
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  }

  // Drag and drop: card to column (not card to card, as in the manual list sorting
  // above) — one dropzone per column, with no manual ordering inside it (sorted by
  // updated_at).
  let boardDragTaskId: string | null = $state(null);
  let boardDropTargetStatus: string | null = $state(null);

  function cardDragStart(e: DragEvent, task: Task) {
    boardDragTaskId = task.id;
    e.dataTransfer?.setData("text/plain", task.id);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function columnDragOver(e: DragEvent, statusId: string) {
    if (!boardDragTaskId) return;
    e.preventDefault();
    boardDropTargetStatus = statusId;
  }

  async function columnDrop(e: DragEvent, statusId: string) {
    e.preventDefault();
    const taskId = boardDragTaskId ?? e.dataTransfer?.getData("text/plain");
    boardDragTaskId = null;
    boardDropTargetStatus = null;
    if (!taskId) return;
    const task = taskStore.tasks.find(t => t.id === taskId);
    if (!task || task.status === statusId) return;
    await moveToStatus(task, statusId);
  }

  // InProgress and Done are special cases with side effects (time tracking,
  // completion) — see api.completeTask/startTaskTracking; every other status,
  // including user-defined ones, is a plain update_task.
  async function moveToStatus(task: Task, statusId: string) {
    if (task.status === "InProgress" && statusId !== "InProgress" && trackingId === task.id) {
      await api.stopTaskTracking();
      trackingId = null;
    }
    if (statusId === "Done") {
      await api.completeTask(task.id);
    } else if (statusId === "InProgress") {
      await api.startTaskTracking(task.id);
      trackingId = task.id;
    } else {
      await api.updateTask(task.id, { status: statusId });
    }
    await taskStore.load();
  }

  let boardCreateStatus = $state("Todo");

  function openBoardCreate(statusId: string) {
    boardCreateStatus = statusId;
    showCreateModal = true;
  }

  // "+ Column" right on the board: a quick way to add a status without going to
  // Settings (renaming and deletion stay there only, see "Task statuses" in
  // Settings.svelte).
  let showStatusQuickAdd = $state(false);
  let newBoardStatusName = $state("");

  async function addBoardStatus() {
    const name = newBoardStatusName.trim();
    if (!name) return;
    await statusStore.create(name, "#888888");
    newBoardStatusName = "";
    showStatusQuickAdd = false;
  }

  // Opening a task on an external signal (global search via Ctrl+K, the day popup
  // in the Dashboard or Calendar). A completed task (hidden) is history, so we open
  // the read-only TaskHistoryDetail rather than the editable TaskModal: otherwise
  // clicking a completed task in the day popup would open it as active for editing,
  // and a deadline or recurrence no longer means anything for something long done.
  $effect(() => {
    const id = taskStore.focusTaskId;
    if (!id) return;
    const task = taskStore.tasks.find(t => t.id === id);
    if (task) {
      if (task.hidden) historyDetailTask = task;
      else editingTask = task;
    }
    taskStore.clearFocus();
  });

  async function handleCreate(data: CreateTaskPayload | UpdateTaskPayload) {
    const payload = data as CreateTaskPayload;
    const created = await taskStore.create(payload);
    // Creating straight into InProgress (via "+ column" on the board, for one): the
    // status is already set by the modal (initialStatus), but the actual tracking
    // timer is started by a separate call, as everywhere else in the app.
    if (created && payload.status === "InProgress") {
      await api.startTaskTracking(created.id);
      trackingId = created.id;
      await taskStore.load();
    }
    return created;
  }

  // --- The inline composer: the first line is the title, Enter inserts a line
  // break, Shift+Enter adds a subtask line (☐), Ctrl+Enter creates the task. ---
  let composerText = $state("");
  let composerEl: HTMLTextAreaElement | undefined = $state();
  let composerBusy = $state(false);
  const composerRows = $derived(Math.min(6, composerText.split("\n").length));

  // Natural language in the title: !priority / @category / #tag and relative
  // dates and times are parsed live from the first line as it is typed.
  const composerDraft = $derived(parseComposer(composerText));
  const composerMeta = $derived(parseTaskText(composerDraft.title));
  const composerCategoryId = $derived(
    composerMeta.categoryQuery ? matchCategoryQuery(categoryStore.categories, composerMeta.categoryQuery) : null
  );

  function composerInsertSubtaskLine() {
    const el = composerEl;
    if (!el) return;
    const start = el.selectionStart;
    const insert = "\n" + SUBTASK_PREFIX;
    composerText = composerText.slice(0, start) + insert + composerText.slice(el.selectionEnd);
    tick().then(() => {
      el.setSelectionRange(start + insert.length, start + insert.length);
    });
  }

  function composerKeydown(e: KeyboardEvent) {
    if (e.key !== "Enter") return;
    if (e.shiftKey) {
      e.preventDefault();
      composerInsertSubtaskLine();
    } else if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      submitComposer();
    }
    // a plain Enter is the default line break
  }

  async function submitComposer() {
    const draft = parseComposer(composerText);
    if (!draft.title || composerBusy) return;
    const meta = parseTaskText(draft.title);
    composerBusy = true;
    try {
      // The active project filter is a sensible default for a new task
      const projectId = projectFilter !== "all" && projectFilter !== "none" ? projectFilter : null;
      const categoryId = meta.categoryQuery ? matchCategoryQuery(categoryStore.categories, meta.categoryQuery) : null;
      const task = await api.createTask({
        title: meta.title || draft.title,
        description: draft.description || null,
        status: "Todo",
        priority: meta.priority ?? "Medium",
        category: categoryId ?? "Other", // the fallback category always exists (Work can be deleted)
        deadline: meta.deadline ? meta.deadline.toISOString() : null,
        tags: meta.tags,
        recurrence: "None",
        project_id: projectId,
      });
      for (const sub of draft.subtasks) {
        await api.addSubtask(task.id, sub);
      }
      composerText = "";
      await taskStore.load();
    } catch (e) {
      aiError = typeof e === "string" ? e : t("Не удалось создать задачу");
    }
    composerBusy = false;
    composerEl?.focus();
  }

  async function handleEdit(data: CreateTaskPayload | UpdateTaskPayload) {
    if (!editingTask) return;
    await taskStore.update(editingTask.id, data as UpdateTaskPayload);
  }

  async function handleSearch() {
    if (!searchQuery.trim()) { searchResults = []; return; }
    isSearching = true;
    searchResults = await taskStore.search(searchQuery);
    isSearching = false;
  }

  async function rewriteTask(id: string, title: string) {
    aiLoadingId = id;
    aiError = null;
    await api.aiRewrite(id, title);
  }

  async function generateSubtasks(id: string, title: string) {
    aiLoadingId = id;
    aiError = null;
    subtasksPreview = null;
    await api.aiSubtasks(id, title);
  }

  // Add a single AI-suggested subtask as a checklist item under its parent task
  async function acceptSubtask(parentId: string, title: string) {
    await api.addSubtask(parentId, title);
    await taskStore.load();
  }

  // Accept every suggested subtask at once
  async function acceptAllSubtasks(parentId: string, items: string[]) {
    for (const title of items) {
      await api.addSubtask(parentId, title);
    }
    subtasksPreview = null;
    await taskStore.load();
  }

  async function toggleSubtask(id: string) {
    await api.toggleSubtask(id);
    await taskStore.load();
  }

  // --- The checklist in a row's panel. The `[x] ` markup is hidden behind a
  // checkbox inside the line, as in the modal and the quick slot — the requirement
  // was to change it the same way everywhere.
  //
  // Writing here is immediate (the panel is opened to tick something and close),
  // but writing to the DB on every keystroke is not an option, hence a typing pause
  // as in the slot. Each task gets its own pause: several rows can be expanded at once.
  const SUBS_DEBOUNCE_MS = 600;
  let subsText = $state<Record<string, string>>({});
  let subsTimers: Record<string, ReturnType<typeof setTimeout>> = {};
  let subsBusy = $state<Record<string, boolean>>({});

  // The panel's text is kept separately from the store: while the user types the
  // store is re-read (by a neighbouring task, for instance) and would clobber the
  // edit. It is initialized on expansion rather than in a $derived.
  function subsTextFor(task: Task): string {
    return subsText[task.id]
      ?? formatChecklist(task.subtasks.map(s => ({ title: s.title, done: s.done })));
  }

  function scheduleSubsFlush(task: Task) {
    clearTimeout(subsTimers[task.id]);
    subsTimers[task.id] = setTimeout(() => flushSubs(task), SUBS_DEBOUNCE_MS);
  }

  // A pending deferred write, i.e. the user has typed something not yet saved.
  // The entry is DELETED rather than only cleared: clearTimeout leaves the id in
  // place, so a plain truthiness check would report "pending" forever after the
  // first edit and freeze the cache permanently.
  function subsPending(id: string): boolean {
    return subsBusy[id] === true || subsTimers[id] !== undefined;
  }

  // The same positional diff as in the modal and the slot: line i edits subtask i.
  // An error is shown as a banner and the text is left as is, so the edit does not
  // disappear silently.
  // Drops the panel's cached text for a task whose checklist the BACKEND changed
  // underneath it.
  //
  // Ticking the last subtask now completes the task on its own
  // (subtasks.rs::complete_if_all_subtasks_done), and for a recurring task that
  // clears every tick as the plan for the next run. The cache knew nothing about
  // it: the screen kept showing ticked boxes over an untitled DB, and the next
  // edit would have flushed those ticks straight back, undoing the reset.
  //
  // Compared against the store rather than cleared on completion, because the
  // reset happens inside the backend and the frontend never sees the moment.
  function dropStaleSubsCache() {
    for (const id of Object.keys(subsText)) {
      const task = taskStore.tasks.find(t => t.id === id);
      // The task is gone from the list (completed, deleted) — nothing to edit.
      if (!task) { delete subsText[id]; continue; }
      // A pending write is the user's own text; it must win until it lands.
      if (subsPending(id)) continue;
      const cached = parseChecklist(subsText[id]);
      const real = task.subtasks;
      const same =
        cached.length === real.length &&
        cached.every((c, i) => c.title === real[i].title && c.done === real[i].done);
      if (!same) delete subsText[id];
    }
  }

  async function flushSubs(task: Task) {
    clearTimeout(subsTimers[task.id]);
    delete subsTimers[task.id];
    if (subsBusy[task.id]) return;
    const current = parseChecklist(subsText[task.id] ?? "");
    const orig = task.subtasks;
    const same =
      current.length === orig.length &&
      current.every((c, i) => c.title === orig[i].title && c.done === orig[i].done);
    if (same) return;
    subsBusy[task.id] = true;
    try {
      await taskStore.guarded(async () => {
        for (let i = current.length; i < orig.length; i++) {
          await api.deleteSubtask(orig[i].id);
        }
        for (let i = 0; i < current.length; i++) {
          const c = current[i];
          const o = orig[i];
          if (!o) {
            const added = await api.addSubtask(task.id, c.title);
            if (c.done) await api.toggleSubtask(added.id);
          } else {
            if (o.title !== c.title) await api.renameSubtask(o.id, c.title);
            if (o.done !== c.done) await api.toggleSubtask(o.id);
          }
        }
      });
      await taskStore.load();
    } finally {
      subsBusy[task.id] = false;
    }
    // After the write lands: the backend may have completed the task on its own
    // (the last subtask was ticked), and for a recurring one that clears the
    // checklist. Runs outside the finally so the flag is already down.
    dropStaleSubsCache();
  }

  let expanded = $state<Record<string, boolean>>({});

  // An explicit click overrides auto-expansion; without one, tasks with subtasks
  // are open when the show_subtasks_expanded setting is on.
  function isExpanded(task: Task): boolean {
    return expanded[task.id] ?? (autoExpandSubs && task.subtasks.length > 0);
  }

  // --- Manual sorting: dragging a row within its own list (group) ---
  let dragTaskId: string | null = $state(null);
  let dropTargetId: string | null = $state(null);

  // --- Multi-select: Ctrl/Shift+click on a row instead of opening the card. Ctrl
  // toggles one row, Shift selects a range from the last selected row within the
  // currently visible list (ignoring grouping — a flat order).
  let selectedIds = $state<Set<string>>(new Set());
  let lastSelectedId: string | null = $state(null);
  let bulkBusy = $state(false);
  let bulkProjectId = $state("");
  let bulkCategory = $state("");

  // The rows in the order they appear on screen. Grouping by project changes that
  // order relative to filteredActive, so a cursor walking the array rather than this
  // would appear to jump around the screen.
  const visibleTasks = $derived.by<Task[]>(() => {
    if (searchQuery.trim()) return searchResults;
    if (grouped) return grouped.flatMap(g => g.tasks);
    return filteredActive;
  });

  function visibleTaskIds(): string[] {
    if (grouped) return grouped.flatMap(g => g.tasks.map(t => t.id));
    return filteredActive.map(t => t.id);
  }

  function toggleSelect(task: Task, e: MouseEvent) {
    const ids = visibleTaskIds();
    if (e.shiftKey && lastSelectedId) {
      const from = ids.indexOf(lastSelectedId);
      const to = ids.indexOf(task.id);
      if (from >= 0 && to >= 0) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        const next = new Set(selectedIds);
        for (let i = lo; i <= hi; i++) next.add(ids[i]);
        selectedIds = next;
        return;
      }
    }
    const next = new Set(selectedIds);
    if (next.has(task.id)) next.delete(task.id); else next.add(task.id);
    selectedIds = next;
    lastSelectedId = task.id;
  }

  // --- Renaming a task in place ---
  //
  // A single click edits the title, a double click opens the modal. The title is
  // the field that gets corrected most often, and going through a modal for a
  // typo was the slow path.
  //
  // The two gestures share the first click, so opening the editor has to wait long
  // enough to find out whether a second one is coming — otherwise every double
  // click would flash an input on its way to the modal.
  let renamingId: string | null = $state(null);
  let renameText = $state("");
  let renameTimer: ReturnType<typeof setTimeout> | null = null;

  function cancelPendingRename() {
    if (renameTimer !== null) {
      clearTimeout(renameTimer);
      renameTimer = null;
    }
  }

  function startRename(task: Task) {
    cancelPendingRename();
    renamingId = task.id;
    renameText = task.title;
  }

  // Saving is deliberately forgiving: Enter and losing focus both commit, the way
  // the quick slot already behaves. An empty title is not a way to delete a task,
  // so it reverts instead of saving.
  async function commitRename() {
    const id = renamingId;
    if (!id) return;
    const task = taskStore.tasks.find(t => t.id === id);
    const next = renameText.trim();
    renamingId = null;
    if (!task || !next || next === task.title) return;
    await taskStore.update(id, { title: next });
  }

  function cancelRename() {
    renamingId = null;
  }

  function onRowClick(e: MouseEvent, task: Task) {
    if (e.ctrlKey || e.metaKey || e.shiftKey) {
      e.preventDefault();
      toggleSelect(task, e);
      return;
    }
    // Already editing this row: a click inside the input must not restart it.
    if (renamingId === task.id) return;
    // Held back until the double-click window passes; ondblclick clears it.
    cancelPendingRename();
    renameTimer = setTimeout(() => {
      renameTimer = null;
      startRename(task);
    }, DBLCLICK_MS);
  }

  function onRowDblClick(e: MouseEvent, task: Task) {
    if (e.ctrlKey || e.metaKey || e.shiftKey) return;
    // Beats the pending single-click editor to it.
    cancelPendingRename();
    renamingId = null;
    editingTask = task;
  }

  function clearSelection() {
    selectedIds = new Set();
    lastSelectedId = null;
  }

  // --- The row's context menu (v0.9.98) ---
  //
  // These six actions used to be .task-actions, revealed on hover. Hover-only made
  // them undiscoverable and unreachable from the keyboard, and on a long list the
  // icons flickered in and out under the moving pointer. A left click still opens
  // the modal — that gesture is older than this menu and stays where it was.
  let rowMenu = $state<{ x: number; y: number; task: Task } | null>(null);

  function openRowMenu(e: MouseEvent, task: Task) {
    e.preventDefault();
    rowMenu = { x: e.clientX, y: e.clientY, task };
  }

  // Two entries carry state rather than just an action, so their label is the
  // state: "Start tracking" against "Stop tracking". The wording is lifted from
  // the titles the icon buttons used, so nothing new needs translating.
  const rowMenuItems = $derived.by(() => {
    const task = rowMenu?.task;
    if (!task) return [];
    const tracking = trackingId === task.id;
    const pinned = pinnedStore.is("task", task.id);
    // The three AI entries share one in-flight flag, the same one the icon
    // buttons used to read.
    const busy = aiLoadingId === task.id;
    return [
      // The modal is a double click now that one click renames. A double click is
      // not a discoverable gesture, so the menu carries the same thing in words.
      { label: t("Открыть карточку"), onSelect: () => { editingTask = task; } },
      { label: t("Переименовать"), separated: true, onSelect: () => startRename(task) },
      { label: t("Переформулировать в SMART"), disabled: busy, onSelect: () => rewriteTask(task.id, task.title) },
      { label: t("Разбить на подзадачи"), disabled: busy, onSelect: () => generateSubtasks(task.id, task.title) },
      { label: t("Авто-категория"), disabled: busy, onSelect: () => classifyTask(task.id, task.title) },
      { label: tracking ? t("Остановить трекинг") : t("Начать трекинг"), separated: true, onSelect: () => toggleTracking(task.id) },
      { label: pinned ? t("Убрать из быстрого слота") : t("В быстрый слот (Ctrl+Shift+J)"), onSelect: () => pinnedStore.toggle("task", task.id) },
      { label: t("Удалить"), danger: true, separated: true, onSelect: () => taskStore.remove(task.id) },
    ];
  });

  // --- Keyboard navigation over the list (v0.9.77) ---
  //
  // Deliberately separate from selectedIds: the cursor is where the keyboard is
  // pointing, the selection is what a bulk action would affect. Merging them would
  // make every j/k silently rewrite the selection.
  let focusedIndex = $state(-1);
  let focusedId: string | null = $state(null);

  // Completing a task removes it from the list and every save re-sorts it, so the
  // cursor is anchored to the row's id and re-derived whenever the list changes.
  $effect(() => {
    const ids = visibleTasks.map(t => t.id);
    const next = reconcileIndex(focusedId, ids, focusedIndex);
    if (next !== focusedIndex) focusedIndex = next;
    focusedId = next >= 0 ? ids[next] : null;
  });

  function moveFocus(delta: number) {
    const next = nextIndex(focusedIndex, delta, visibleTasks.length);
    focusedIndex = next;
    focusedId = next >= 0 ? visibleTasks[next].id : null;
    if (next >= 0) {
      const el = document.querySelector<HTMLElement>(`[data-task-index="${next}"]`);
      el?.scrollIntoView({ block: "nearest" });
    }
  }

  function onListKeydown(e: KeyboardEvent) {
    // Ctrl+Tab toggles List/Board. Handled before every guard below: those exist to
    // protect the list cursor, but this shortcut belongs to the screen and has to
    // work from the board too — which the cursor guards would otherwise block.
    // A modal still wins: Tab inside one is moving between its fields.
    if (e.ctrlKey && !e.altKey && e.code === "Tab" && !editingTask && !historyDetailTask) {
      e.preventDefault();
      viewMode = viewMode === "list" ? "board" : "list";
      return;
    }

    // Only the active list has a cursor. In History and the Trash the actions would
    // mean something else (Delete there is "purge forever"), and the board is
    // two-dimensional — j/k cannot express a move across columns.
    if (viewMode !== "list" || listSubView !== "active") return;
    // A modal on top owns the keyboard; otherwise Escape would clear the cursor
    // behind a card the user is actually closing.
    if (editingTask || historyDetailTask) return;
    if (isTypingTarget(document.activeElement)) return;

    const action = actionForKey(e);
    if (!action) return;

    if (action === "down" || action === "up") {
      e.preventDefault();
      moveFocus(action === "down" ? 1 : -1);
      return;
    }
    if (action === "escape") {
      focusedIndex = -1;
      focusedId = null;
      return;
    }

    const task = focusedIndex >= 0 ? visibleTasks[focusedIndex] : null;
    if (!task) return;
    e.preventDefault();
    if (action === "open") {
      // Enter renames, matching what one click now does with the mouse. The modal
      // is the double click, or "Редактировать" in the row's context menu.
      startRename(task);
    } else if (action === "complete") {
      // The blocked-task prohibition lives in the backend (v0.9.56) and the row's
      // checkmark is disabled — the keyboard must not become the one path that
      // bypasses both and produces an error instead of doing nothing.
      if (task.blocked_by.length === 0) completeRow(task);
    } else if (action === "delete") {
      taskStore.remove(task.id);
    }
  }

  async function bulkComplete() {
    bulkBusy = true;
    try {
      await Promise.all([...selectedIds].map(id => api.completeTask(id)));
      await taskStore.load();
      clearSelection();
    } finally {
      bulkBusy = false;
    }
  }

  async function bulkDelete() {
    bulkBusy = true;
    try {
      await Promise.all([...selectedIds].map(id => api.deleteTask(id)));
      await taskStore.load();
      clearSelection();
    } finally {
      bulkBusy = false;
    }
  }

  async function bulkMoveToProject() {
    if (!bulkProjectId) return;
    bulkBusy = true;
    try {
      const project_id = bulkProjectId === "none" ? "" : bulkProjectId;
      await Promise.all([...selectedIds].map(id => api.updateTask(id, { project_id })));
      await taskStore.load();
      clearSelection();
      bulkProjectId = "";
    } finally {
      bulkBusy = false;
    }
  }

  async function bulkSetCategory() {
    if (!bulkCategory) return;
    bulkBusy = true;
    try {
      await Promise.all([...selectedIds].map(id => api.updateTask(id, { category: bulkCategory as Category })));
      await taskStore.load();
      clearSelection();
      bulkCategory = "";
    } finally {
      bulkBusy = false;
    }
  }

  function listForTask(task: Task): Task[] {
    if (grouped) {
      const g = grouped.find(g => g.tasks.some(t => t.id === task.id));
      return g ? g.tasks : [];
    }
    return filteredActive;
  }

  function rowDragStart(e: DragEvent, task: Task) {
    dragTaskId = task.id;
    e.dataTransfer?.setData("text/plain", task.id);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function rowDragOver(e: DragEvent, task: Task) {
    if (!dragTaskId || dragTaskId === task.id) return;
    e.preventDefault();
    dropTargetId = task.id;
  }

  async function rowDrop(e: DragEvent, target: Task) {
    e.preventDefault();
    const sourceId = dragTaskId ?? e.dataTransfer?.getData("text/plain");
    dragTaskId = null;
    dropTargetId = null;
    if (!sourceId || sourceId === target.id) return;
    const ids = listForTask(target).map(t => t.id);
    const from = ids.indexOf(sourceId);
    const to = ids.indexOf(target.id);
    if (from < 0 || to < 0) return; // dragging between groups is not sorting
    ids.splice(from, 1);
    ids.splice(to, 0, sourceId);
    await taskStore.reorder(ids);
  }
  const doneCount = (t: Task) => t.subtasks.filter((s) => s.done).length;

  async function classifyTask(id: string, title: string) {
    aiLoadingId = id;
    aiError = null;
    await api.aiClassify(id, title);
  }

  const PRIORITY_LABELS: Record<string, string> = $derived({
    Low: t("Низкий"), Medium: t("Средний"), High: t("Высокий"), Critical: t("Критический"),
  });

  function recurrenceLabel(r: unknown): string | null {
    if (!r || r === "None") return null;
    if (r === "Hourly") return t("Каждый час");
    if (r === "Daily")  return t("Каждый день");
    if (r === "Weekly") return t("Каждую неделю");
    if (typeof r === "object" && r !== null && "Custom" in r) {
      const [n, unit] = (r as any).Custom;
      const unitLabel =
        unit === "Minutes" ? t("мин.") :
        unit === "Hours"   ? t("ч.") :
        unit === "Days"    ? t("дн.") : t("нед.");
      return t("раз в {n} {unit}", { n, unit: unitLabel });
    }
    if (typeof r === "object" && r !== null && "Weekdays" in r) {
      const labels = [t("Пн"), t("Вт"), t("Ср"), t("Чт"), t("Пт"), t("Сб"), t("Вс")];
      const mask = (r as any).Weekdays as number;
      const days = labels.filter((_, i) => mask & (1 << i));
      return t("по {days}", { days: days.join(", ") });
    }
    return null;
  }

  // A compact deadline: "today 18:00", "tomorrow", "3 d", "2 d overdue"
  function deadlineInfo(iso: string): { label: string; overdue: boolean } {
    const d = new Date(iso);
    const now = new Date();
    const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const dayDiff = Math.round((startOfDay(d) - startOfDay(now)) / 864e5);

    if (d.getTime() < now.getTime()) {
      return { label: dayDiff === 0 ? t("просрочено") : t("просрочено {n} дн", { n: -dayDiff }), overdue: true };
    }
    if (dayDiff === 0) {
      return { label: t("сегодня {time}", { time: d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) }), overdue: false };
    }
    if (dayDiff === 1) return { label: t("завтра"), overdue: false };
    if (dayDiff < 7) return { label: t("{n} дн", { n: dayDiff }), overdue: false };
    return { label: d.toLocaleDateString([], { day: "numeric", month: "short" }), overdue: false };
  }

  taskStore.load();

  onMount(() => {
    const unlistenAi = listen<AiResult>("ai-result", async ({ payload }) => {
      if (payload.error) {
        aiLoadingId = null;
        aiError = payload.error;
        return;
      }
      if (!payload.result) { aiLoadingId = null; return; }

      if (payload.type === "rewrite") {
        await taskStore.update(payload.task_id, { title: payload.result });
        aiLoadingId = null;
      } else if (payload.type === "subtasks") {
        const items = payload.result.split("|||").filter(Boolean);
        subtasksPreview = { taskId: payload.task_id, items };
        aiLoadingId = null;
      } else if (payload.type === "classify") {
        const valid = ["Work","Study","Home","Health","Other"];
        if (valid.includes(payload.result)) {
          await taskStore.update(payload.task_id, { category: payload.result as Category });
        }
        aiLoadingId = null;
      }
    });

    const unlistenWhatNow = listen<{ result: string | null; error: string | null }>("ai-what-now", ({ payload }) => {
      whatNowPending = false;
      whatNow = payload.result;
      if (payload.error) aiError = payload.error;
    });

    return () => {
      unlistenAi.then(fn => fn());
      unlistenWhatNow.then(fn => fn());
    };
  });

  // "What should I do now": AI advice from the current context (blocks, deadlines, priorities)
  let whatNow: string | null = $state(null);
  let whatNowPending = $state(false);

  async function askWhatNow() {
    whatNowPending = true;
    whatNow = null;
    aiError = null;
    try {
      await api.aiWhatNow();
    } catch (e) {
      whatNowPending = false;
      aiError = String(e);
    }
  }
</script>

<svelte:window onkeydown={onListKeydown} />

<!-- One resize handle. Rendered between every pair of columns and once more
     after the last one, always driving the column named here. Its own element
     rather than a border on .column, because a 1px border is far too small a
     target to grab. -->
{#snippet colHandle(target: { id: string })}
  <!-- role=slider, not separator: this one is focusable and driven by the arrow
       keys, and a separator is non-interactive by definition — the a11y lint
       flags a tabindex on it for exactly that reason. A slider is what a
       draggable value with a min/max actually is. -->
  <div
    class="col-resize"
    role="slider"
    aria-label={t("Ширина колонки «{name}»", { name: statusStore.name(target.id) })}
    aria-orientation="vertical"
    aria-valuenow={widthOf(target.id)}
    aria-valuemin={COL_MIN}
    aria-valuemax={COL_MAX}
    tabindex="0"
    onpointerdown={(e) => startColResize(e, target.id)}
    onkeydown={(e) => {
      // Keyboard is not a nicety here: a pointer drag is the only other way to
      // reach this, and it is unusable without a mouse.
      const step = e.shiftKey ? 40 : 10;
      if (e.key === "ArrowLeft") { e.preventDefault(); setColWidth(target.id, widthOf(target.id) - step); saveUiState({ boardColWidths: colWidths }); }
      else if (e.key === "ArrowRight") { e.preventDefault(); setColWidth(target.id, widthOf(target.id) + step); saveUiState({ boardColWidths: colWidths }); }
    }}
  ></div>
{/snippet}

{#snippet taskRow(task: Task)}
  {@const busy = aiLoadingId === task.id}
  {@const blocked = task.blocked_by.length > 0}
  {@const blockerNames = task.blocked_by.map(b => b.title).join(", ")}
  <!-- The keyboard cursor is matched by id rather than by a positional index passed
       in: the snippet is rendered from three different branches (search, grouped,
       flat), and only the flat one has an index that matches the screen order. -->
  {@const kbIndex = visibleTasks.findIndex(v => v.id === task.id)}
  {@const unblocksCount = unblocks.get(task.id) ?? 0}
  <li
    class="task-row"
    style="--prio: var(--prio-{task.priority.toLowerCase()});"
    class:dragging={dragTaskId === task.id}
    class:drop-target={dropTargetId === task.id}
    class:selected={selectedIds.has(task.id)}
    class:kb-focused={kbIndex >= 0 && kbIndex === focusedIndex}
    data-task-index={kbIndex}
    class:blocked
    draggable={!searchQuery.trim() && !task.hidden}
    ondragstart={(e) => rowDragStart(e, task)}
    ondragover={(e) => rowDragOver(e, task)}
    ondrop={(e) => rowDrop(e, task)}
    ondragend={() => { dragTaskId = null; dropTargetId = null; }}
    oncontextmenu={(e) => openRowMenu(e, task)}
  >
    <!-- A blocked task cannot be completed. The backend forbids it too, but
         disabled here keeps a click from producing an error. -->
    <button
      class="task-check"
      onclick={() => completeRow(task)}
      disabled={blocked}
      title={blocked ? t("Заблокирована: {tasks}", { tasks: blockerNames }) : t("Выполнить")}
      aria-label={t("Выполнить задачу")}
    ></button>

    <div
      class="task-main"
      onclick={(e) => onRowClick(e, task)}
      ondblclick={(e) => onRowDblClick(e, task)}
      onkeydown={(e) => { if (e.key === "Enter") startRename(task); }}
      role="button"
      tabindex="0"
    >
      {#if renamingId === task.id}
        <input
          class="task-title-edit"
          bind:value={renameText}
          {@attach (el) => {
            // Focus from an attachment rather than the autofocus attribute: the
            // browser only honours autofocus for the first such element after a
            // page load, so it worked when the editor was opened from the keyboard
            // and silently did nothing when opened by a click.
            el.focus();
            el.select();
          }}
          onblur={commitRename}
          onkeydown={(e) => {
            // stopPropagation, not just preventDefault: the row's own onkeydown
            // sits on the ancestor .task-main and would catch this same Enter and
            // start the rename over, throwing away what was just typed.
            if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); commitRename(); }
            // Stops the list's own Escape handler from also clearing the selection.
            else if (e.key === "Escape") { e.stopPropagation(); cancelRename(); }
          }}
          aria-label={t("Название задачи")}
        />
      {:else}
        <div class="task-title" title="{t('Приоритет')}: {PRIORITY_LABELS[task.priority]}">
          {task.title}
          {#if recurrenceLabel(task.recurrence)}
            <span class="muted" title={recurrenceLabel(task.recurrence)}>↻</span>
          {/if}
        </div>
      {/if}
      {#if task.description}
        <div class="task-desc">{task.description}</div>
      {/if}
      <!-- The reason is spelled out rather than only dimmed: otherwise it is
           unclear why the task's checkmark will not click. -->
      {#if blocked}
        <div class="task-blocked-by">{t("Заблокирована: {tasks}", { tasks: blockerNames })}</div>
      {/if}
      <!-- The other direction: what finishing this task frees up. It is the answer
           to "which one do I take first", so it sits on the blocker's own row. -->
      {#if unblocksCount > 0}
        <div class="task-unblocks" title={t("Эту задачу ждут другие — выполните её, чтобы снять блокировку")}>
          {t("разблокирует {count}", { count: unblocksCount })}
        </div>
      {/if}
    </div>

    <div class="task-meta">
      <button
        class="chip chip-sub"
        class:has-subs={task.subtasks.length > 0}
        class:subs-done={task.subtasks.length > 0 && doneCount(task) === task.subtasks.length}
        onclick={() => expanded[task.id] = !isExpanded(task)}
        title={task.subtasks.length > 0 ? t("Подзадачи") : t("Добавить подзадачу")}
      >{isExpanded(task) ? "▾" : "▸"}
        {#if task.subtasks.length > 0}
          <span class="sub-track"><span class="sub-fill" style="width:{Math.round(doneCount(task) / task.subtasks.length * 100)}%"></span></span>
          {doneCount(task)}/{task.subtasks.length}
        {:else}+{/if}</button>
      {#each task.tags as tag}
        <span class="chip chip-tag">#{tag}</span>
      {/each}
      <!-- Outlined rather than filled: up to four chips sit side by side on a row,
           and the solid weight is reserved for the active category filter. -->
      <span class="chip chip-cat chip-cat--edge" style="--cat: {categoryStore.color(task.category)}">{categoryStore.name(task.category)}</span>
      {#if task.deadline}
        {@const dl = deadlineInfo(task.deadline)}
        <span class="chip" class:chip-danger={dl.overdue}><Icon name="flag" size={11} /> {dl.label}</span>
      {/if}
    </div>

  </li>

  {#if subtasksPreview && subtasksPreview.taskId === task.id}
    <li class="task-sub-panel">
      <div class="sub-preview-head">
        <span class="section-title" style="margin:0;">{t("ИИ предлагает подзадачи")}</span>
        <div style="display:flex;gap:6px;">
          <button class="btn-sm btn-primary" onclick={() => acceptAllSubtasks(task.id, subtasksPreview!.items)}>{t("Принять все")}</button>
          <button class="btn-sm" onclick={() => subtasksPreview = null}>{t("Закрыть")}</button>
        </div>
      </div>
      {#each subtasksPreview.items as subtask}
        <div class="sub-line">
          <span style="flex:1;">{subtask}</span>
          <button class="btn-sm" onclick={() => acceptSubtask(task.id, subtask)}>{t("+ Добавить")}</button>
        </div>
      {/each}
    </li>
  {/if}

  {#if isExpanded(task)}
    <li class="task-sub-panel">
      <ChecklistEditor
        value={subsTextFor(task)}
        placeholder={t("Подзадача на строку (Enter — ещё строка)")}
        onchange={(text) => { subsText[task.id] = text; scheduleSubsFlush(task); }}
      />
    </li>
  {/if}
{/snippet}

<!-- Modals -->
{#if showCreateModal}
  <TaskModal
    initialStatus={boardCreateStatus}
    onSave={handleCreate}
    onClose={() => showCreateModal = false}
  />
{/if}

{#if editingTask}
  <TaskModal
    task={editingTask}
    onSave={handleEdit}
    onClose={() => editingTask = null}
  />
{/if}

{#if rowMenu}
  <ContextMenu
    x={rowMenu.x}
    y={rowMenu.y}
    items={rowMenuItems}
    onClose={() => rowMenu = null}
  />
{/if}

{#if historyDetailTask}
  <TaskHistoryDetail
    task={historyDetailTask}
    onClose={() => historyDetailTask = null}
  />
{/if}

{#if showProjects}
  <div role="dialog" aria-modal="true" class="overlay backdrop"
    onclick={(e) => { if (e.target === e.currentTarget) showProjects = false; }}>
    <div class="modal dialog">
      <h2 class="dialog-title">{t("Проекты")}</h2>

      {#if projectStore.error}
        <div class="alert" style="margin:0;">{tErr(projectStore.error)}</div>
      {/if}

      {#each projectStore.projects as p (p.id)}
        <div class="proj-row" class:archived={p.archived}>
          <input
            value={p.name}
            onchange={(e) => projectStore.update(p.id, { name: e.currentTarget.value })}
          />
          <span class="muted proj-progress">{p.task_done}/{p.task_total}</span>
          <button class="btn-sm" title={p.archived ? t("Разархивировать") : t("В архив")}
            onclick={() => projectStore.update(p.id, { archived: !p.archived })}>
            {p.archived ? t("Вернуть") : t("Архив")}
          </button>
          <button class="btn-icon btn-danger" title={t("Удалить проект (задачи останутся без проекта)")}
            onclick={() => projectStore.remove(p.id)}>✕</button>
        </div>
        {#if !p.archived}
          <div class="proj-goal">
            <span class="muted">{t("Цель:")}</span>
            <input class="goal-num" type="number" min="0" placeholder="—"
              value={p.goal_tasks ?? ""}
              onchange={(e) => projectStore.update(p.id, { goal_tasks: Number(e.currentTarget.value) || 0 })}
            />
            <span class="muted">{t("задач ·")}</span>
            <input class="goal-num" type="number" min="0" step="15" placeholder="—"
              value={p.goal_mins ?? ""}
              onchange={(e) => projectStore.update(p.id, { goal_mins: Number(e.currentTarget.value) || 0 })}
            />
            <span class="muted">{t("мин в")}</span>
            <select
              value={p.goal_period}
              onchange={(e) => projectStore.update(p.id, { goal_period: e.currentTarget.value as "week" | "month" })}
            >
              <option value="week">{t("неделю")}</option>
              <option value="month">{t("месяц")}</option>
            </select>
            {#if goalText(p)}
              <span class="goal-chip" class:met={goalMet(p)}>{goalText(p)}</span>
              <button class="btn-sm" onclick={() => toggleGoalHistory(p.id)}>
                {showGoalHistory[p.id] ? t("Скрыть") : t("История")}
              </button>
            {/if}
            {#if showGoalHistory[p.id]}
              <div class="goal-history">
                {#if goalHistoryLoading[p.id]}
                  <span class="muted">{t("Загрузка…")}</span>
                {:else if showGoalHistory[p.id].length === 0}
                  <span class="muted">{t("Нет записей")}</span>
                {:else}
                  {#each showGoalHistory[p.id] as snap (snap.id)}
                    <div class="goal-history-row">
                      <span class="muted">{snap.recorded_at.slice(0, 16)}</span>
                      <span>{snap.done_tasks}{snap.goal_tasks != null ? `/${snap.goal_tasks}` : ''} {t("задач")}</span>
                      <span>·</span>
                      <span>{snap.done_mins}{snap.goal_mins != null ? `/${snap.goal_mins}` : ''} {t("мин")}</span>
                    </div>
                  {/each}
                {/if}
              </div>
            {/if}
          </div>
        {/if}
      {:else}
        <p class="muted" style="margin:0;font-size:13px;">{t("Проектов пока нет — создайте первый.")}</p>
      {/each}

      <div class="proj-row">
        <input
          bind:value={newProjectName}
          placeholder={t("Название нового проекта")}
          onkeydown={(e) => { if (e.key === "Enter") addProject(); }}
        />
        <button class="btn-primary" onclick={addProject} disabled={!newProjectName.trim()}>{t("Создать")}</button>
      </div>

      <div class="actions">
        <button class="btn-ghost" onclick={() => showProjects = false}>{t("Закрыть")}</button>
      </div>
    </div>
  </div>
{/if}

{#if showSmartListModal}
  <div role="dialog" aria-modal="true" class="overlay backdrop"
    onclick={(e) => { if (e.target === e.currentTarget) { showSmartListModal = false; resetSmartListForm(); } }}>
    <div class="modal dialog">
      <h2 class="dialog-title">{t("Новый умный список")}</h2>

      {#if smartListStore.error}
        <div class="alert" style="margin:0;">{tErr(smartListStore.error)}</div>
      {/if}

      <label class="field">
        <span class="label">{t("Название")}</span>
        <input bind:value={newSmartListName} placeholder={t("Например: Важное")} />
      </label>

      <div class="pair" style="margin-top:8px;">
        <label class="field">
          <span class="label">{t("Категория")}</span>
          <select bind:value={newSmartListCategory}>
            <option value="">{t("Любая")}</option>
            {#each categoryStore.categories as c (c.id)}
              <option value={c.id}>{categoryStore.name(c.id)}</option>
            {/each}
          </select>
        </label>
        <label class="field">
          <span class="label">{t("Приоритет")}</span>
          <select bind:value={newSmartListPriority}>
            <option value="">{t("Любой")}</option>
            {#each Object.entries(PRIORITY_LABELS) as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        </label>
      </div>

      <div class="pair" style="margin-top:8px;">
        <label class="field">
          <span class="label">{t("Тег")}</span>
          <input bind:value={newSmartListTag} placeholder={t("без #")} />
        </label>
        <label class="field">
          <span class="label">{t("Дедлайн")}</span>
          <select bind:value={newSmartListHasDeadline}>
            <option value="">{t("Не важно")}</option>
            <option value="yes">{t("Есть дедлайн")}</option>
            <option value="no">{t("Без дедлайна")}</option>
          </select>
        </label>
      </div>

      <p class="hint">{t("Условия комбинируются через «И» — задача должна подойти под все заданные.")}</p>

      <div class="actions">
        <button class="btn-ghost" onclick={() => { showSmartListModal = false; resetSmartListForm(); }}>{t("Отмена")}</button>
        <button class="btn-primary" onclick={createSmartList} disabled={!newSmartListName.trim()}>{t("Создать")}</button>
      </div>
    </div>
  </div>
{/if}

<div class="page" class:board-mode={viewMode === "board"}>
  <div class="page-head">
    <h1 class="page-title">{t("Задачи")}</h1>
    <span class="muted count">
      {t("{active} актив. · {history} в истории", {
        active: taskStore.activeTasks.length,
        history: taskStore.historyTasks.length,
      })}
    </span>
    <div class="seg">
      <button class:active={viewMode === "list"} onclick={() => viewMode = "list"}>{t("Список")}</button>
      <button class:active={viewMode === "board"} onclick={() => viewMode = "board"}>{t("Доска")}</button>
    </div>
    <span style="flex:1;"></span>
    <input
      bind:value={searchQuery}
      oninput={handleSearch}
      placeholder={t("Поиск задач…")}
      class="head-search"
    />
    {#if projectStore.projects.length > 0}
      <select bind:value={projectFilter} class="project-filter" title={t("Фильтр по проекту")}>
        <option value="all">{t("Все проекты")}</option>
        <option value="none">{t("Без проекта")}</option>
        {#each projectStore.active as p (p.id)}
          <option value={p.id}>{p.name}</option>
        {/each}
      </select>
    {/if}
    {#if aiEnabled}
      <button onclick={askWhatNow} disabled={whatNowPending}
        title={t("ИИ посоветует, чем заняться сейчас — по блокам, дедлайнам и приоритетам")}>
        {#if whatNowPending}{t("Думаю…")}{:else}<Icon name="target" size={12} /> {t("Что сейчас?")}{/if}
      </button>
    {/if}
    <button onclick={() => { showProjects = true; projectStore.load(); }}>{t("Проекты")}</button>
    <div class="seg">
      <button class:active={listSubView === "active"} onclick={() => listSubView = "active"}>{t("Активные")}</button>
      <button class:active={listSubView === "history"} onclick={() => listSubView = "history"}>{t("История")}</button>
      <button class:active={listSubView === "trash"} onclick={() => { listSubView = "trash"; taskStore.loadDeleted(); }}>{t("Корзина")}</button>
    </div>
    <button class="btn-primary" onclick={() => { boardCreateStatus = "Todo"; showCreateModal = true; }}>{t("+ Новая")}</button>
  </div>

  <!-- Store errors are finally visible. taskStore.error used to be set but never
       rendered anywhere, so a failed operation looked like "the button does not
       work", with no sign that anything had gone wrong (that is exactly how the
       recurrence bug presented). This is the same inline .alert already used in
       Notes and Settings. -->
  {#if taskStore.error}
    <div class="alert task-error" role="alert">
      <span>{tErr(taskStore.error)}</span>
      <button class="btn-sm" onclick={() => taskStore.clearError()} title={t("Скрыть")}>✕</button>
    </div>
  {/if}

  {#if selectedIds.size > 0}
    <div class="bulk-bar card">
      <span class="bulk-count">{t("{n} выбрано", { n: selectedIds.size })}</span>
      <select bind:value={bulkProjectId} disabled={bulkBusy} title={t("Перенести в проект")}>
        <option value="" disabled selected>{t("В проект…")}</option>
        <option value="none">{t("Без проекта")}</option>
        {#each projectStore.active as p (p.id)}
          <option value={p.id}>{p.name}</option>
        {/each}
      </select>
      {#if bulkProjectId}
        <button class="btn-sm" disabled={bulkBusy} onclick={bulkMoveToProject}>{t("Перенести")}</button>
      {/if}
      <select bind:value={bulkCategory} disabled={bulkBusy} title={t("Сменить категорию")}>
        <option value="" disabled selected>{t("Категория…")}</option>
        {#each categoryStore.categories as c (c.id)}
          <option value={c.id}>{categoryStore.name(c.id)}</option>
        {/each}
      </select>
      {#if bulkCategory}
        <button class="btn-sm" disabled={bulkBusy} onclick={bulkSetCategory}>{t("Применить")}</button>
      {/if}
      <button class="btn-sm" disabled={bulkBusy} onclick={bulkComplete}>{t("Выполнить")}</button>
      <button class="btn-sm btn-danger" disabled={bulkBusy} onclick={bulkDelete}>{t("Удалить")}</button>
      <span style="flex:1;"></span>
      <button class="btn-icon" title={t("Снять выбор")} onclick={clearSelection}>✕</button>
    </div>
  {/if}

  {#if aiError}
    <div class="ai-error">
      <span>{tErr(aiError)}</span>
      <button class="btn-icon" style="color:white;" onclick={() => aiError = null}>✕</button>
    </div>
  {/if}

  {#if whatNow}
    <div class="what-now card">
      <span class="what-now-icon"><Icon name="target" size={16} /></span>
      <span class="what-now-text">{whatNow}</span>
      <button class="btn-icon" onclick={() => whatNow = null}>✕</button>
    </div>
  {/if}

  {#if viewMode === "board"}
    <!-- The board reports its own distance from the top of the window, so the
         columns can reach the bottom edge exactly. It cannot be a constant: the
         header wraps at narrow widths, and the filter chips, the error bar and
         the bulk-selection bar all appear and disappear above the board. -->
    <div
      class="board"
      class:resizing
      {@attach (el) => {
        const sync = () => el.style.setProperty("--board-top", `${Math.round(el.getBoundingClientRect().top)}px`);
        sync();
        // Observed rather than measured once: everything above the board can change
        // height without the board itself re-rendering.
        const ro = new ResizeObserver(sync);
        ro.observe(el);
        ro.observe(document.documentElement);
        window.addEventListener("resize", sync);
        return () => { ro.disconnect(); window.removeEventListener("resize", sync); };
      }}
    >
      {#each boardColumns as col, i (col.id)}
        {#if i > 0}
          {@render colHandle(boardColumns[i - 1])}
        {/if}
        <div
          class="column"
          style="--col-w: {widthOf(col.id)}px"
          role="list"
          class:drop-target={boardDropTargetStatus === col.id}
          ondragover={(e) => columnDragOver(e, col.id)}
          ondrop={(e) => columnDrop(e, col.id)}
          ondragleave={() => { if (boardDropTargetStatus === col.id) boardDropTargetStatus = null; }}
        >
          <div class="column-head">
            <span class="column-title" style="--cat: {col.color}">{statusStore.name(col.id)}</span>
            <span class="muted column-count">{boardTasksFor(col.id).length}</span>
            <button class="btn-icon" title={t("Новая задача")} onclick={() => openBoardCreate(col.id)}>+</button>
          </div>

          <div class="column-body">
            {#each boardTasksFor(col.id) as task (task.id)}
              {@const cardBlocked = task.blocked_by.length > 0}
              <button
                class="board-card"
                style="--prio: var(--prio-{task.priority.toLowerCase()});"
                title={cardBlocked
                  ? t("Заблокирована: {tasks}", { tasks: task.blocked_by.map(b => b.title).join(", ") })
                  : `${t('Приоритет')}: ${PRIORITY_LABELS[task.priority]}`}
                class:blocked={cardBlocked}
                class:dragging={boardDragTaskId === task.id}
                draggable="true"
                ondragstart={(e) => cardDragStart(e, task)}
                ondragend={() => { boardDragTaskId = null; boardDropTargetStatus = null; }}
                onclick={() => editingTask = task}
                oncontextmenu={(e) => openRowMenu(e, task)}
              >
                <div class="board-card-title">
                  {task.title}
                  {#if trackingId === task.id}
                    <span class="tracking-dot" title={t("Идёт трекинг")}><Icon name="play" size={10} /></span>
                  {/if}
                </div>
                <div class="board-card-meta">
                  <span class="chip chip-cat chip-cat--edge" style="--cat: {categoryStore.color(task.category)}">{categoryStore.name(task.category)}</span>
                  <!-- Only the count, without the list's progress bar and without
                       the "+" for an empty task: in the list that chip is a button
                       that unfolds the subtask editor below the row, and a card has
                       nowhere to unfold into. So it stays a plain chip — a card that
                       has no subtasks simply says nothing. -->
                  {#if task.subtasks.length > 0}
                    <span
                      class="chip chip-subs"
                      class:subs-done={doneCount(task) === task.subtasks.length}
                      title={t("Подзадачи")}
                    >{doneCount(task)}/{task.subtasks.length}</span>
                  {/if}
                  {#if task.deadline}
                    {@const dl = deadlineInfo(task.deadline)}
                    <span class="chip" class:chip-danger={dl.overdue}><Icon name="flag" size={10} /> {dl.label}</span>
                  {/if}
                  {#each task.tags as tag}
                    <span class="chip chip-tag">#{tag}</span>
                  {/each}
                </div>
              </button>
            {:else}
              <p class="empty-col muted">{t("Пусто")}</p>
            {/each}
          </div>
        </div>
      {/each}
      {#if boardColumns.length > 0}
        <!-- The last column's handle, on its right. Without it that column was the
             one you could not resize: the handles sit between columns and each one
             drives the column to its left, so the rightmost had nothing after it. -->
        {@render colHandle(boardColumns[boardColumns.length - 1])}
      {/if}
      <div class="add-column">
        <button class="btn-sm" onclick={() => showStatusQuickAdd = true}>{t("+ Колонка")}</button>
        {#if showStatusQuickAdd}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            bind:value={newBoardStatusName}
            placeholder={t("Название статуса")}
            autofocus
            onkeydown={(e) => { if (e.key === "Enter") addBoardStatus(); if (e.key === "Escape") { showStatusQuickAdd = false; newBoardStatusName = ""; } }}
            onblur={() => { if (!newBoardStatusName.trim()) showStatusQuickAdd = false; }}
          />
        {/if}
      </div>
    </div>
  {:else}
  {#if listSubView === "active"}
  {#if todayBlocks.length > 0 && !searchQuery.trim()}
    <div class="day-plan card">
      <span class="day-plan-label">{t("Сегодня:")}</span>
      {#each todayBlocks as t (t.id)}
        <button class="chip day-plan-chip" onclick={() => editingTask = t} title={t.title}>
          <span class="day-plan-time">{blockTime(t)}</span> {t.title}
        </button>
      {/each}
    </div>
  {/if}

  {#if !searchQuery.trim()}
    <div class="smart-lists">
      <button
        class="chip smart-list-chip"
        class:active-toggle={activeSmartListId === null}
        onclick={() => activeSmartListId = null}
      >{t("Все")}</button>
      {#each BUILTIN_SMART_LISTS as l (l.id)}
        <button
          class="chip smart-list-chip"
          class:active-toggle={activeSmartListId === l.id}
          onclick={() => activeSmartListId = activeSmartListId === l.id ? null : l.id}
        >{t(l.name)}</button>
      {/each}
      {#each smartListStore.lists as l (l.id)}
        <span class="chip smart-list-chip custom" class:active-toggle={activeSmartListId === l.id}>
          <button class="smart-list-name" onclick={() => activeSmartListId = activeSmartListId === l.id ? null : l.id}>{l.name}</button>
          <button class="smart-list-remove" title={t("Удалить список")} onclick={() => removeSmartList(l.id)}>✕</button>
        </span>
      {/each}
      <button class="chip smart-list-chip smart-list-add" title={t("Создать умный список")} onclick={() => showSmartListModal = true}>{t("+ Список")}</button>
    </div>

    <!-- The category filter (v0.9.99). A separate row from the smart lists above:
         those are saved queries, these are one attribute of a task, and merging
         them would suggest picking one cancels the other. -->
    {#if categoryStore.categories.length > 0}
      <div class="cat-filter">
        <button
          class="chip"
          class:chip-cat--solid={categoryFilter === "all"}
          style={categoryFilter === "all" ? `--cat: var(--text-secondary); --on-cat: var(--bg-card)` : ""}
          onclick={() => categoryFilter = "all"}
        >{t("Все")}</button>
        {#each categoryStore.categories as c (c.id)}
          {@const on = categoryFilter === c.id}
          <button
            class="chip chip-cat"
            class:chip-cat--solid={on}
            class:chip-cat--edge={!on}
            style="--cat: {c.color}; --on-cat: {onAccentText(c.color)}"
            onclick={() => categoryFilter = on ? "all" : c.id}
          >{categoryStore.name(c.id)}</button>
        {/each}
      </div>
    {/if}
  {/if}

  {#if !searchQuery.trim()}
    <div class="composer card">
      <textarea
        class="composer-input"
        bind:this={composerEl}
        bind:value={composerText}
        onkeydown={composerKeydown}
        rows={composerRows}
        placeholder={t("Быстрая задача… (!приоритет @категория #тег, завтра 15:00 — Shift+Enter подзадача, Ctrl+Enter создать)")}
      ></textarea>
      {#if composerDraft.title}
        <button class="btn-primary btn-sm composer-send" disabled={composerBusy} onclick={submitComposer}>
          {composerBusy ? "…" : t("Создать")}
        </button>
      {/if}
    </div>
    {#if composerDraft.title && (composerMeta.priority || composerMeta.categoryQuery || composerMeta.tags.length > 0 || composerMeta.deadline)}
      <div class="composer-preview">
        {#if composerMeta.priority}
          <span class="chip" style="--prio: var(--prio-{composerMeta.priority.toLowerCase()});">
            <span class="prio-dot"></span> {PRIORITY_LABELS[composerMeta.priority]}
          </span>
        {/if}
        {#if composerMeta.categoryQuery}
          {#if composerCategoryId}
            <span class="chip chip-cat" style="--cat: {categoryStore.color(composerCategoryId)}">{categoryStore.name(composerCategoryId)}</span>
          {:else}
            <span class="chip chip-danger" title={t("Категория «{q}» не найдена — будет «Другое»", { q: composerMeta.categoryQuery })}>@{composerMeta.categoryQuery} ?</span>
          {/if}
        {/if}
        {#each composerMeta.tags as tag}
          <span class="chip chip-tag">#{tag}</span>
        {/each}
        {#if composerMeta.deadline}
          <span class="chip"><Icon name="flag" size={11} /> {composerMeta.deadline.toLocaleString([], { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}</span>
        {/if}
      </div>
    {/if}
  {/if}

  {#if searchQuery.trim()}
    <div class="section-title">{t("Результаты поиска")}</div>
    {#if isSearching}
      <div class="empty">{t("Поиск…")}</div>
    {:else if searchResults.length === 0}
      <div class="empty">{t("Ничего не найдено")}</div>
    {:else}
      <ul class="task-list card">
        {#each searchResults as task (task.id)}
          {@render taskRow(task)}
        {/each}
      </ul>
    {/if}
  {:else}
    {#if taskStore.activeTasks.length === 0}
      <div class="empty card">
        {t("Нет активных задач.")}<br />
        <span class="muted">{t("Создайте первую: «+ Новая» или Ctrl+Shift+N")}</span>
      </div>
    {:else if filteredActive.length === 0}
      <div class="empty card">{activeSmartListId ? t("В этом списке нет задач") : t("В этом проекте нет активных задач")}</div>
    {:else if grouped}
      {#each grouped as group (group.id)}
        <div class="section-title project-head">
          <span>{group.name}</span>
          {#if group.total > 0}
            <span class="muted">{group.done}/{group.total}</span>
          {/if}
          {#if group.project}
            {@const goal = goalText(group.project)}
            {#if goal}
              <span class="goal-chip" class:met={goalMet(group.project)}
                title={group.project.goal_period === "month" ? t("Цель месяца") : t("Цель недели")}>
                {goal}
              </span>
            {/if}
          {/if}
        </div>
        <ul class="task-list card" style="margin-bottom:12px;">
          {#each group.tasks as task (task.id)}
            {@render taskRow(task)}
          {/each}
        </ul>
      {/each}
    {:else}
      <ul class="task-list card">
        {#each filteredActive as task (task.id)}
          {@render taskRow(task)}
        {/each}
      </ul>
    {/if}
  {/if}

  {:else if listSubView === "history"}
    <div class="empty-hint">
      {t("✓ Выполненные задачи. Повторяющиеся не попадают сюда — они остаются активными.")}
    </div>
    {#if taskStore.historyTasks.length === 0}
      <div class="empty card">{t("История пуста")}</div>
    {:else}
      <ul class="task-list card history">
        {#each taskStore.historyTasks as task (task.id)}
          <li class="task-row">
            <span class="task-check done history-icon">✓</span>
            <div
              class="task-main"
              onclick={() => historyDetailTask = task}
              onkeydown={(e) => { if (e.key === "Enter") historyDetailTask = task; }}
              role="button"
              tabindex="0"
            >
              <div class="task-title done-title">{task.title}</div>
              {#if task.description}
                <div class="task-desc">{task.description}</div>
              {/if}
            </div>
            <div class="task-meta">
              {#if task.subtasks.length > 0}
                <span class="chip">{doneCount(task)}/{task.subtasks.length}</span>
              {/if}
              <span class="chip">{statusStore.name(task.status)}</span>
            </div>
            <div class="task-actions">
              <button class="btn-icon btn-danger" title={t("Удалить")} onclick={() => taskStore.remove(task.id)}>✕</button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}

  {:else}
    <div class="empty-hint trash-hint">
      {t("🗑 Удалённые задачи. Восстановить можно в любой момент, пока не нажато «Удалить навсегда».")}
    </div>
    {#if taskStore.deletedTasks.length === 0}
      <div class="empty card">{t("Корзина пуста")}</div>
    {:else}
      <ul class="task-list card trash">
        {#each taskStore.deletedTasks as task (task.id)}
          <li class="task-row">
            <span class="task-check trash-icon">🗑</span>
            <div class="task-main">
              <div class="task-title done-title">{task.title}</div>
              {#if task.description}
                <div class="task-desc">{task.description}</div>
              {/if}
            </div>
            <div class="task-meta">
              {#if task.subtasks.length > 0}
                <span class="chip">{doneCount(task)}/{task.subtasks.length}</span>
              {/if}
            </div>
            <div class="task-actions">
              <button class="btn-sm" title={t("Восстановить")} onclick={() => taskStore.restore(task.id)}>{t("Восстановить")}</button>
              <button class="btn-icon btn-danger" title={t("Удалить навсегда")} onclick={() => taskStore.purge(task.id)}>✕</button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
  {/if}
</div>

<style>
  /* The shell for the two dialogs in this view — Projects and the new smart list.
     Both carried class="modal dialog" while only .modal was ever defined (in
     app.css), so they got a card with no padding: the title sat 1px from the
     border and the buttons ran into the edges, which read as a window cut off
     rather than a dialog.

     The numbers match .dialog in TaskModal, which is what these two sit next to.
     Deliberately duplicated rather than lifted into app.css: five components
     declare their own .dialog and four differ only in max-width and gap, so
     merging them is its own change, not a side effect of this fix. */
  .dialog {
    width: 100%;
    max-width: 500px;
    max-height: 90vh;
    overflow-y: auto;
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .dialog-title {
    margin: 0;
    font-size: 15px;
    font-weight: 700;
  }

  /* Buttons on the trailing edge, pushed down off the content above. */
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }

  /* The cap sits on the content below the header, not on .page itself. It used to
     be on .page, which made the header inherit it: in List mode the whole bar —
     search, "Projects", the Active/History/Trash switch, "+ New" — stopped 1200px
     in and left an empty strip on both sides, while in Board mode (1600px) it ran
     to the window edge. The same bar in the same place changed width when the view
     below it changed, which is what showed up in the screenshots.

     The reason for a cap at all is the task rows: a row stretched across a wide
     monitor puts its title and its chips absurdly far apart. That argument is
     about the rows, so the cap belongs to them. */
  .page {
    margin: 0 auto;
  }

  /* Left-aligned inside .page, never `margin: 0 auto`. Centring each child made
     them line up with nothing: .task-list is not a direct child, so it kept the
     page's left edge at 196px while the chips and the composer — which are —
     were centred to 288px. Three different left edges on one screen. Everything
     under the header starts where the header starts; only the width is capped. */
  .page > * {
    max-width: 1200px;
    margin-right: auto;
  }

  /* The board is wider than the list: several columns in a row do not fit into
     the narrow task-list container. */
  .page.board-mode > * {
    max-width: 1600px;
  }

  /* The header is the one child that opts out, so it spans the window in both
     modes. Written as an override rather than as `> :not(.page-head)` because
     viewWidth.test.ts reads the ceiling out of the `.page > *` rule by name;
     a :not() selector hid it and the guard reported Tasks as having no ceiling
     at all.

     It must stay below the .board-mode rule: that selector has the same
     specificity, so whichever comes last wins. Placed above, the board re-capped
     the header at 1600px and the widths diverged again past a 1600px window —
     the same defect, just further out. */
  .page > .page-head {
    max-width: none;
  }

  .page-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
    flex-wrap: wrap;
  }

  .count { font-size: 12px; }

  /* .alert sets the background, colour and padding globally (app.css); only the
     layout for the close button is here. */
  .task-error {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .task-error span { flex: 1; }

  .head-search {
    width: 200px;
  }

  .active-toggle {
    background: var(--bg-hover);
    font-weight: 600;
  }

  .project-filter {
    max-width: 160px;
  }

  .project-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .proj-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }

  .proj-row input {
    flex: 1;
    min-width: 0;
  }

  .proj-row.archived input {
    opacity: 0.55;
    text-decoration: line-through;
  }

  .proj-progress {
    font-size: 12px;
    flex-shrink: 0;
  }

  .proj-goal {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    margin: -4px 0 10px 8px;
    flex-wrap: wrap;
  }

  .proj-goal .goal-num {
    width: 58px;
    padding: 3px 6px;
    font-size: 12px;
  }

  .proj-goal select {
    padding: 3px 6px;
    font-size: 12px;
  }

  .goal-chip {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    background: var(--bg-hover);
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .goal-chip.met {
    background: color-mix(in srgb, var(--success) 15%, transparent);
    color: var(--success);
    font-weight: 600;
  }

  .goal-history {
    width: 100%;
    font-size: 11px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 0 0 8px;
  }

  .goal-history-row {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .day-plan {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 8px 12px;
    margin-bottom: 12px;
  }

  .task-row.dragging { opacity: 0.5; }
  .task-row.drop-target { box-shadow: inset 0 2px 0 var(--accent); }
  .task-row.selected {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    box-shadow: inset 3px 0 0 var(--accent);
  }

  /* An outline rather than a fill or a left bar: multi-select already uses both, and
     a row can be selected and under the cursor at the same time. */
  .task-row.kb-focused {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .bulk-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 8px 12px;
    margin-bottom: 12px;
  }

  .bulk-count {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
  }

  .composer {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 8px 12px;
    margin-bottom: 12px;
  }

  .composer-input {
    flex: 1;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    padding: 2px 0;
  }
  .composer-input:focus { outline: none; }

  .composer-send { flex-shrink: 0; }

  .composer-preview {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 0 12px 10px;
    margin-top: -8px;
    margin-bottom: 12px;
  }

  .what-now {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    border-left: 3px solid var(--accent);
    font-size: 13px;
  }

  .what-now-text { flex: 1; }

  .day-plan-label {
    font-size: 12px;
    color: var(--text-secondary);
    font-weight: 600;
  }

  .day-plan-chip {
    max-width: 260px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .day-plan-time {
    color: var(--accent);
    font-weight: 600;
  }

  .smart-lists {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }

  /* The category filter row. Sits tighter under the smart lists than they sit
     under each other, so the two read as related but distinct controls. */
  .cat-filter {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    margin: -4px 0 12px;
  }

  .cat-filter .chip {
    cursor: pointer;
  }

  .smart-list-chip {
    cursor: pointer;
    border: none;
  }

  .smart-list-chip.custom {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding-right: 4px;
    cursor: default;
  }

  .smart-list-name {
    border: none;
    background: transparent;
    padding: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
  }

  .smart-list-remove {
    border: none;
    background: transparent;
    padding: 0 2px;
    font-size: 10px;
    color: var(--text-secondary);
    cursor: pointer;
    line-height: 1;
  }

  .smart-list-remove:hover {
    color: var(--danger);
  }

  .smart-list-add {
    color: var(--text-secondary);
    background: transparent;
    border: 1px dashed var(--border);
  }

  .pair {
    display: flex;
    gap: 10px;
  }

  .pair .field {
    flex: 1;
  }

  .hint {
    font-size: 11px;
    color: var(--text-secondary);
    margin: 8px 0 0 0;
  }

  /* --- The board --- */
  .board {
    display: flex;
    /* No gap: the resize handle sits in each gap and provides the spacing itself.
       With both, the columns would drift 12px further apart per divider. */
    gap: 0;
    align-items: flex-start;
    overflow-x: auto;
    padding-bottom: 8px;
  }

  /* The grab area is 12px wide — the same space the gap used to be — while the
     visible line inside it is 1px. A handle only as wide as its line would be a
     1px target. */
  .col-resize {
    flex: 0 0 12px;
    align-self: stretch;
    cursor: col-resize;
    position: relative;
    background: none;
    border: none;
    padding: 0;
  }

  .col-resize::before {
    content: "";
    position: absolute;
    top: 8px;
    bottom: 8px;
    left: 50%;
    width: 1px;
    background: var(--border);
    transform: translateX(-50%);
    transition: background 0.12s, width 0.12s;
  }

  .col-resize:hover::before,
  .col-resize:focus-visible::before,
  .board.resizing .col-resize::before {
    background: var(--accent);
    width: 3px;
  }

  .col-resize:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  /* While dragging, the cursor is the resize arrow everywhere and text stops
     selecting — otherwise a pull across a column highlights every card title. */
  .board.resizing {
    cursor: col-resize;
    user-select: none;
  }

  .column {
    flex: 0 0 var(--col-w, 260px);
    display: flex;
    flex-direction: column;
    background: var(--bg-secondary);
    border-radius: var(--radius);
    border: 1px solid var(--border);
    /* Fill the window down to its bottom edge, rather than stopping at a guessed
       height. This was `calc(100vh - 220px)`: 220 stood for everything above the
       board, but the real offset depends on the header wrapping, the filter chips
       and the error and bulk bars, which appear and disappear. Measured at a
       940px window it left the column ending at 794 with 146px of empty page
       below — and those 146px were unreachable, because the cards are scrolled by
       .column-body inside the column, not by the page.

       The 8px keeps .board's own padding-bottom off the window edge. */
    max-height: calc(100vh - var(--board-top, 74px) - 8px);
  }

  .column.drop-target {
    box-shadow: inset 0 0 0 2px var(--accent);
  }

  .column-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
  }

  .column-title {
    font-weight: 600;
    font-size: 13px;
    color: var(--cat, var(--text-primary));
  }

  .column-count {
    font-size: 12px;
  }

  .column-head .btn-icon {
    margin-left: auto;
  }

  .column-body {
    flex: 1;
    overflow-y: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .empty-col {
    font-size: 12px;
    text-align: center;
    margin: 12px 0;
  }

  .board-card {
    display: block;
    width: 100%;
    text-align: left;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    /* 13 = the row's own 10 minus the 3 the edge occupies, so the text keeps the
       same distance from the card's content edge as before. */
    padding: 8px 10px 8px 13px;
    cursor: pointer;
    font: inherit;
    color: inherit;
    position: relative;
  }

  /* The same priority edge as .task-row, for the same reason: as a dot it sat
     inside .board-card-title and pushed every title right by its own width plus
     the flex gap, so titles started at different offsets down a column. On the
     edge they line up and the colour reads without looking at each card. */
  .board-card::before {
    content: "";
    position: absolute;
    left: 0;
    top: 3px;
    bottom: 3px;
    width: 3px;
    border-radius: 0 3px 3px 0;
    background: var(--prio, var(--prio-low));
  }

  .board-card:hover {
    background: var(--bg-hover);
  }

  .board-card.dragging {
    opacity: 0.5;
  }

  /* Same colours as the list's .chip-sub, without its button behaviour: accent
     while there is work left, green once everything is ticked. */
  .chip-subs {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    font-weight: 600;
  }
  .chip-subs.subs-done {
    color: var(--success);
    background: color-mix(in srgb, var(--success) 12%, transparent);
  }

  /* Blocked, dimmed the same way as .task-row.blocked and for the same reason:
     the contents fade, never the card itself, or the priority edge would fade
     with them. The blocker's name is in the card's title attribute rather than
     spelled out as a line — the list has the row's full width for that, while a
     column is narrow enough that the extra line would make cards jump in height. */
  .board-card.blocked .board-card-title,
  .board-card.blocked .board-card-meta { opacity: .55; }

  .board-card-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 500;
    margin-bottom: 6px;
  }

  .tracking-dot {
    margin-left: auto;
    color: var(--accent);
    display: inline-flex;
  }

  .board-card-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .add-column {
    flex: 0 0 180px;
  }

  .add-column input {
    width: 100%;
    margin-top: 4px;
  }

  .ai-error {
    background: var(--danger);
    color: white;
    padding: 6px 10px;
    border-radius: var(--radius);
    margin-bottom: 10px;
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .task-list {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: hidden;
  }

  .task-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 12px 7px 15px;
    border-bottom: 1px solid var(--border);
    position: relative;
  }

  /* Priority as the row's left edge rather than a dot before the title: the dot
     sat inside .task-title, so titles started at different offsets and the
     colour only read from close up. On the edge the titles line up and the
     colour is scannable down the whole list.

     The composer preview keeps .prio-dot: it names the priority in words next to
     it, so the dot is a swatch for that label rather than a mark on a task. The
     board card took the same edge (v0.10.03). */
  .task-row::before {
    content: "";
    position: absolute;
    left: 0;
    top: 3px;
    bottom: 3px;
    width: 3px;
    border-radius: 0 3px 3px 0;
    background: var(--prio, var(--prio-low));
  }

  .task-list > .task-row:last-child,
  .task-list > .task-sub-panel:last-child {
    border-bottom: none;
  }

  .task-row:hover {
    background: var(--bg-hover);
  }

  /* The round completion checkbox */
  .task-check {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    padding: 0;
    border-radius: 50%;
    border: 1.5px solid var(--text-secondary);
    background: transparent;
    color: transparent;
    font-size: 10px;
    line-height: 1;
  }

  .task-check:hover {
    border-color: var(--success);
    background: color-mix(in srgb, var(--success) 15%, transparent);
    color: var(--success);
  }

  .task-check.done {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-color: var(--success);
    color: var(--success);
    cursor: default;
  }

  .task-main {
    flex: 1;
    min-width: 0;
    cursor: pointer;
  }

  .task-title {
    font-size: 13px;
    font-weight: 500;
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* The rename field stands where the title was and keeps its type, so the row
     does not jump when the editor opens. Only the frame says it is editable:
     matching the title's own size and weight is what makes this read as editing
     the row rather than as a form appearing inside it. */
  .task-title-edit {
    width: 100%;
    font-size: 13px;
    font-weight: 500;
    padding: 1px 6px;
    /* Cancels the 5px 10px from app.css, which would push the row taller than its
       neighbours for as long as the editor is open. */
    margin: -2px 0;
  }

  .done-title {
    color: var(--text-secondary);
    text-decoration: line-through;
    font-weight: 400;
  }

  .prio-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--prio, var(--prio-low));
  }

  /* A blocked task is dimmed but readable: it stays in the list so it is not
     forgotten. Only the row's contents are dimmed, not the row itself — an
     opacity on .task-row would also mute the coloured priority edge on the
     left, which is what makes the list scannable. */
  .task-row.blocked .task-main,
  .task-row.blocked .task-meta { opacity: .55; }
  .task-row.blocked .task-check { cursor: not-allowed; }

  .task-blocked-by {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Deliberately louder than .task-blocked-by: being blocked is a passive state to
     read, while blocking others is a call to act — this is the row to pick up. */
  .task-unblocks {
    font-size: 11px;
    color: var(--accent);
    margin-top: 2px;
  }

  .task-desc {
    font-size: 12px;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin-top: 1px;
  }

  .task-meta {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-shrink: 0;
  }

  .chip-sub {
    cursor: pointer;
    border: none;
    font-family: inherit;
  }
  .chip-sub:hover { background: var(--bg-hover); }

  /* A task WITH subtasks looks different from an empty "+": an accent chip with
     a mini progress bar, turning green once they are all done. */
  .chip-sub.has-subs {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    font-weight: 600;
  }
  .chip-sub.has-subs:hover { background: color-mix(in srgb, var(--accent) 20%, transparent); }
  .chip-sub.subs-done {
    color: var(--success);
    background: color-mix(in srgb, var(--success) 12%, transparent);
  }
  .chip-sub.subs-done:hover { background: color-mix(in srgb, var(--success) 20%, transparent); }

  .sub-track {
    width: 26px;
    height: 4px;
    border-radius: 2px;
    background: color-mix(in srgb, currentColor 25%, transparent);
    overflow: hidden;
  }
  .sub-fill {
    display: block;
    height: 100%;
    background: currentColor;
  }

  /* Only History and the Trash still use .task-actions, and there it is a single
     button per row. The active list moved its six actions into the row's context
     menu (v0.9.98) — hover-only made them undiscoverable and unreachable from the
     keyboard, which a one-button row does not suffer from nearly as much. */
  .task-actions {
    display: flex;
    gap: 1px;
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.12s;
  }

  .task-row:hover .task-actions {
    opacity: 1;
  }

  /* The subtasks panel / AI preview below the row */
  .task-sub-panel {
    list-style: none;
    padding: 6px 12px 8px 38px;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .sub-preview-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 4px;
  }

  .sub-line {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  /* The checklist rows moved into ChecklistEditor: striking through completed
     items and the field styling live there now. .sub-line remains — it is used
     by the AI suggestion rows above. */

  /* No priority edge in History or the Trash. Those rows never set --prio, so
     the edge would fall back to --prio-low and paint every finished or deleted
     task a grey stripe that means "low priority" — a signal about a task no
     longer being worked on. */
  .history .task-row::before,
  .trash .task-row::before {
    display: none;
  }

  .history .task-row {
    opacity: 0.75;
    padding-left: 12px;
  }

  /* The Trash uses the same muted row as History but with an explicit red accent
     on the icon, so "completed" and "deleted" are not confused visually (both
     used to share the same green .task-check.done). */
  .trash .task-row {
    opacity: 0.75;
    padding-left: 12px;
  }

  .trash-icon {
    border-color: var(--danger) !important;
    color: var(--danger) !important;
  }

  .empty-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-secondary);
    margin-bottom: 10px;
  }
</style>
