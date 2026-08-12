<script lang="ts">
  import { onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { taskStore } from "../lib/stores/tasks.svelte";
  import { routineStore } from "../lib/stores/routines.svelte";
  import { api } from "../lib/api/tauri";
  import TaskModal from "../lib/components/TaskModal.svelte";
  import TaskOpener from "../lib/components/TaskOpener.svelte";
  import RoutinesModal from "../lib/components/RoutinesModal.svelte";
  import Icon from "../lib/components/Icon.svelte";
  import type { Task, CreateTaskPayload, RoutineBlock } from "../lib/types";
  // `t` is taken in this file by the task variable in {#each}, so the translation
  // helper is imported as `tr` — renaming the loops for a function's sake is worse.
  import { t as tr, tErr, i18n } from "../lib/i18n.svelte";
  import { localDateKey, hhmm, hhmmFromMins, pad2, localeTag } from "../lib/datetime";

  // A task opens right here rather than by switching to the Tasks screen: in the
  // calendar one looks at the week as a whole, and leaving for another section over
  // a single task broke that.
  let openTaskId = $state<string | null>(null);

  const today = new Date();
  let year = $state(today.getFullYear());
  let month = $state(today.getMonth()); // 0-11
  let viewMode = $state<"month" | "week">("month");

  // The AI planner: proposed blocks (ghosts in the grid) before "Apply"
  interface PlannedBlock { id: string; title: string; scheduled_at: string; mins: number }
  let planning = $state(false);
  let proposed: PlannedBlock[] | null = $state(null);
  let planError: string | null = $state(null);

  let aiEnabled = $state(false);
  let showRoutinesModal = $state(false);

  // The "plan my day" signal from the command palette (Ctrl+K): switch to the week
  // and start the planner, just like the "⚡ Plan my day" button.
  let planDayKey = $state(0);
  $effect(() => {
    planDayKey;
    if (taskStore.planDayRequested === 0) return;
    planDayKey = taskStore.planDayRequested;
    viewMode = "week";
    if (aiEnabled) planDay();
  });

  onMount(() => {
    taskStore.load();
    routineStore.load();
    // Capability detection: with AI turned off the planner is simply hidden
    api.getSettings().then(s => aiEnabled = s.ai_provider !== "none").catch(() => {});
    const unlisteners: UnlistenFn[] = [];
    (async () => {
      unlisteners.push(await listen<{ blocks: PlannedBlock[]; error: string | null }>("ai-plan", (e) => {
        planning = false;
        planError = e.payload.error;
        proposed = e.payload.error ? null : e.payload.blocks;
      }));
    })();
    return () => unlisteners.forEach(u => u());
  });

  async function planDay() {
    planning = true;
    planError = null;
    proposed = null;
    try {
      await api.aiPlanDay();
    } catch (e) {
      planning = false;
      planError = String(e);
    }
  }

  async function applyPlan() {
    if (!proposed) return;
    for (const b of proposed) {
      await taskStore.update(b.id, { scheduled_at: b.scheduled_at, scheduled_mins: b.mins });
    }
    proposed = null;
  }

  // Ghosts per day (a plan is always for today, but we lay them out generically)
  const proposedByDay = $derived.by(() => {
    const map = new Map<string, PlannedBlock[]>();
    for (const b of proposed ?? []) {
      const key = localDateKey(new Date(b.scheduled_at));
      map.set(key, [...(map.get(key) ?? []), b]);
    }
    return map;
  });

  function ghostTop(b: PlannedBlock): number {
    const d = new Date(b.scheduled_at);
    return ((d.getHours() * 60 + d.getMinutes()) / 60) * HOUR_H;
  }

  function ghostLabel(b: PlannedBlock): string {
    const start = new Date(b.scheduled_at);
    const end = new Date(start.getTime() + b.mins * 60_000);
    return `${hhmm(start)}–${hhmm(end)}`;
  }

  // Month and weekday names come from Intl rather than from the dictionary. These
  // are calendar data, not interface strings: putting 19 entries into i18n.en.ts
  // would mean maintaining by hand what the platform already knows for both
  // languages (and would know for a third one, should it appear). A $derived so the
  // list is rebuilt when the language changes, without a reload.
  const locale = $derived(localeTag(i18n.lang));
  const MONTHS = $derived(
    Array.from({ length: 12 }, (_, m) => {
      const name = new Intl.DateTimeFormat(locale, { month: "long" }).format(new Date(2021, m, 1));
      return name.charAt(0).toUpperCase() + name.slice(1);
    })
  );
  // The week starts on Monday (2021-03-01 was one), as everywhere in the app; Intl
  // by itself does not define the order of days.
  const WEEKDAYS = $derived(
    Array.from({ length: 7 }, (_, i) => {
      const name = new Intl.DateTimeFormat(locale, { weekday: "short" })
        .format(new Date(2021, 2, 1 + i));
      return name.charAt(0).toUpperCase() + name.slice(1).replace(/\.$/, "");
    })
  );

  // Tasks by the local date of their deadline (hidden history items are not shown).
  const tasksByDay = $derived.by(() => {
    const map = new Map<string, Task[]>();
    for (const t of taskStore.activeTasks) {
      if (!t.deadline) continue;
      const key = localDateKey(new Date(t.deadline));
      const list = map.get(key) ?? [];
      list.push(t);
      map.set(key, list);
    }
    return map;
  });

  interface DayCell {
    key: string;
    day: number;
    inMonth: boolean;
    isToday: boolean;
    tasks: Task[];
  }

  // The month grid: weeks starting on Monday, always full rows of 7.
  const grid = $derived.by(() => {
    const first = new Date(year, month, 1);
    const lead = (first.getDay() + 6) % 7; // how many days of the previous month to show
    const start = new Date(year, month, 1 - lead);
    const todayKey = localDateKey(new Date());

    const cells: DayCell[] = [];
    const d = new Date(start);
    do {
      const key = localDateKey(d);
      cells.push({
        key,
        day: d.getDate(),
        inMonth: d.getMonth() === month,
        isToday: key === todayKey,
        tasks: tasksByDay.get(key) ?? [],
      });
      d.setDate(d.getDate() + 1);
    } while (d.getMonth() === month || cells.length % 7 !== 0);
    return cells;
  });

  function shiftMonth(delta: number) {
    if (viewMode === "week") {
      weekAnchor = new Date(weekAnchor.getFullYear(), weekAnchor.getMonth(), weekAnchor.getDate() + delta * 7);
      return;
    }
    const d = new Date(year, month + delta, 1);
    year = d.getFullYear();
    month = d.getMonth();
  }

  function goToday() {
    year = today.getFullYear();
    month = today.getMonth();
    weekAnchor = new Date();
  }

  // ===== Week: time blocking =====
  const HOUR_H = 44; // px per hour
  const SNAP_MIN = 15;

  let weekAnchor = $state(new Date());

  function mondayOf(d: Date): Date {
    const day = (d.getDay() + 6) % 7;
    return new Date(d.getFullYear(), d.getMonth(), d.getDate() - day);
  }

  const weekDays = $derived.by(() => {
    const start = mondayOf(weekAnchor);
    const todayKey = localDateKey(new Date());
    return Array.from({ length: 7 }, (_, i) => {
      const d = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i);
      const key = localDateKey(d);
      return { key, date: d, label: `${WEEKDAYS[i]} ${d.getDate()}`, isToday: key === todayKey };
    });
  });

  const weekLabel = $derived.by(() => {
    const start = mondayOf(weekAnchor);
    const end = new Date(start.getFullYear(), start.getMonth(), start.getDate() + 6);
    return `${start.getDate()} ${MONTHS[start.getMonth()].slice(0, 3).toLowerCase()} — ${end.getDate()} ${MONTHS[end.getMonth()].slice(0, 3).toLowerCase()} ${end.getFullYear()}`;
  });

  // An in-progress block resize: a live preview without saving on every pixel
  let resizing: { id: string; mins: number } | null = $state(null);

  function blockMins(t: Task): number {
    if (resizing && resizing.id === t.id) return resizing.mins;
    return t.scheduled_mins ?? 60;
  }

  // Blocks by weekday (only tasks that are not hidden)
  const blocksByDay = $derived.by(() => {
    const map = new Map<string, Task[]>();
    for (const t of taskStore.activeTasks) {
      if (!t.scheduled_at) continue;
      const key = localDateKey(new Date(t.scheduled_at));
      const list = map.get(key) ?? [];
      list.push(t);
      map.set(key, list);
    }
    return map;
  });

  // Idle time within blocks: task_id -> idle minutes. Loaded only for the visible
  // week and only in week mode — the month view shows no blocks, so there is
  // nothing to request.
  //
  // The past and present, not the future: a block in the future has no idle time by
  // definition, so zero values mean nothing and are not shown (see the
  // idle_mins > 0 filter in the markup).
  let blockIdle = $state(new Map<string, number>());

  async function loadBlockIdle() {
    if (viewMode !== "week") return;
    const map = new Map<string, number>();
    for (const d of weekDays) {
      const rows = await api.getBlockIdle(d.key).catch(() => []);
      for (const r of rows) {
        if (r.idle_mins > 0) map.set(r.task_id, r.idle_mins);
      }
    }
    blockIdle = map;
  }

  // Reloaded when the week changes and when switching into week mode.
  $effect(() => {
    void viewMode;
    void weekAnchor;
    loadBlockIdle();
  });

  // Routines by weekday: for each day of the week we check the mask
  const routinesByDay = $derived.by(() => {
    const map = new Map<string, { title: string; start_mins: number; duration_mins: number }[]>();
    for (const d of weekDays) {
      const dayOfWeek = d.date.getDay() === 0 ? 6 : d.date.getDay() - 1; // 0 = Monday
      const blocks: RoutineBlock[] = [];
      for (const r of routineStore.active) {
        if (r.days_mask & (1 << dayOfWeek)) {
          blocks.push({ title: r.title, start_mins: r.start_mins, duration_mins: r.duration_mins });
        }
      }
      if (blocks.length > 0) map.set(d.key, blocks);
    }
    return map;
  });

  // The backlog: active tasks with no block (Todo/InProgress)
  const backlog = $derived(
    taskStore.activeTasks.filter(t => !t.scheduled_at && (t.status === "Todo" || t.status === "InProgress"))
  );

  function blockTop(t: Task): number {
    const d = new Date(t.scheduled_at!);
    return ((d.getHours() * 60 + d.getMinutes()) / 60) * HOUR_H;
  }

  function blockLabel(t: Task): string {
    const start = new Date(t.scheduled_at!);
    const end = new Date(start.getTime() + blockMins(t) * 60_000);
    return `${hhmm(start)}–${hhmm(end)}`;
  }

  function snap(mins: number): number {
    return Math.round(mins / SNAP_MIN) * SNAP_MIN;
  }

  // --- Drag and drop (HTML5): backlog to slot, block to another slot ---
  // dataTransfer holds only the id; the grab offset lives in a module variable.
  let dragOffsetY = 0;

  function onBlockDragStart(e: DragEvent, t: Task) {
    e.dataTransfer?.setData("text/plain", t.id);
    dragOffsetY = e.offsetY;
  }

  function onBacklogDragStart(e: DragEvent, t: Task) {
    e.dataTransfer?.setData("text/plain", t.id);
    dragOffsetY = 0;
  }

  async function onDayDrop(e: DragEvent, dayKey: string) {
    e.preventDefault();
    const id = e.dataTransfer?.getData("text/plain");
    if (!id) return;
    const task = taskStore.tasks.find(t => t.id === id);
    if (!task) return;

    const col = e.currentTarget as HTMLElement;
    const y = e.clientY - col.getBoundingClientRect().top - dragOffsetY;
    const mins = Math.max(0, Math.min(24 * 60 - SNAP_MIN, snap((y / HOUR_H) * 60)));
    const [yy, mm, dd] = dayKey.split("-").map(Number);
    const start = new Date(yy, mm - 1, dd, Math.floor(mins / 60), mins % 60);

    await taskStore.update(id, {
      scheduled_at: start.toISOString(),
      scheduled_mins: task.scheduled_mins ?? 60,
    });
  }

  async function unschedule(id: string) {
    await taskStore.update(id, { scheduled_at: "" });
  }

  // --- Resizing by the bottom edge ---
  function startResize(e: MouseEvent, t: Task) {
    e.preventDefault();
    e.stopPropagation();
    const startY = e.clientY;
    const startMins = t.scheduled_mins ?? 60;
    resizing = { id: t.id, mins: startMins };

    const move = (ev: MouseEvent) => {
      const delta = ((ev.clientY - startY) / HOUR_H) * 60;
      resizing = { id: t.id, mins: Math.max(SNAP_MIN, snap(startMins + delta)) };
    };
    const up = async () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      const mins = resizing?.mins ?? startMins;
      resizing = null;
      if (mins !== startMins) await taskStore.update(t.id, { scheduled_mins: mins });
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }

  // On entering week mode we scroll the grid to 8 in the morning
  let weekScrollEl: HTMLDivElement | undefined = $state();
  $effect(() => {
    if (viewMode === "week" && weekScrollEl) {
      weekScrollEl.scrollTop = 8 * HOUR_H;
    }
  });

  function chipClass(t: Task): string {
    if (t.status === "Done" || t.status === "Archived") return "done";
    if (t.deadline && new Date(t.deadline) < new Date()) return "overdue";
    return "";
  }

  const MAX_CHIPS = 3;

  // Clicking a day creates a task with a deadline on it (the cell's key).
  let createFor = $state<string | null>(null);

  async function handleCreate(data: unknown) {
    return await taskStore.create(data as CreateTaskPayload);
  }
</script>

<div class="cal">
  <div class="page-head">
    <h2 class="page-title">{tr("Календарь")}</h2>
    <div class="seg mode-toggle">
      <button class:active={viewMode === "month"} onclick={() => viewMode = "month"}>{tr("Месяц")}</button>
      <button class:active={viewMode === "week"} onclick={() => viewMode = "week"}>{tr("Неделя")}</button>
    </div>
    <span style="flex:1;"></span>
    <button class="btn-icon" onclick={() => shiftMonth(-1)} title={viewMode === "week" ? tr("Предыдущая неделя") : tr("Предыдущий месяц")}>←</button>
    <span class="month-label">{viewMode === "week" ? weekLabel : `${MONTHS[month]} ${year}`}</span>
    <button class="btn-icon" onclick={() => shiftMonth(1)} title={viewMode === "week" ? tr("Следующая неделя") : tr("Следующий месяц")}>→</button>
    <button class="btn-sm" onclick={goToday}>{tr("Сегодня")}</button>
    {#if viewMode === "week"}
      <button class="btn-sm" onclick={() => showRoutinesModal = true}>{tr("Рутины")}</button>
    {/if}
  </div>

  {#if viewMode === "week"}
  <div class="week-layout">
    <div class="week-main card">
      <div class="week-head">
        <div class="hour-gutter-head"></div>
        {#each weekDays as d (d.key)}
          <div class="week-day-head" class:today={d.isToday}>{d.label}</div>
        {/each}
      </div>

      <div class="week-scroll" bind:this={weekScrollEl}>
        <div class="week-grid" style="height:{24 * HOUR_H}px;">
          <div class="hour-gutter">
            {#each Array(24) as _, h}
              <div class="hour-mark" style="height:{HOUR_H}px;">{pad2(h)}:00</div>
            {/each}
          </div>

          {#each weekDays as d (d.key)}
            <div
              class="week-col"
              class:today={d.isToday}
              role="list"
              ondragover={(e) => e.preventDefault()}
              ondrop={(e) => onDayDrop(e, d.key)}
              style="background-size: 100% {HOUR_H}px;"
            >
              {#each blocksByDay.get(d.key) ?? [] as t (t.id)}
                <div
                  class="block"
                  role="listitem"
                  draggable="true"
                  ondragstart={(e) => onBlockDragStart(e, t)}
                  style="top:{blockTop(t)}px; height:{Math.max((blockMins(t) / 60) * HOUR_H, 18)}px;"
                  title="{blockLabel(t)} · {t.title}"
                >
                  <button class="block-body" onclick={() => openTaskId = t.id}>
                    <span class="block-time">{blockLabel(t)}</span>
                    <span class="block-title">{t.title}</span>
                    <!-- Idle time: shown only when there is any. Zero is not drawn —
                         for future blocks it is zero by definition, and "0 min idle"
                         would read as a statement of fact. -->
                    {#if blockIdle.get(t.id)}
                      <span class="block-idle" title={tr("Простой внутри блока по данным мониторинга")}>
                        {tr("простой {n} мин", { n: blockIdle.get(t.id) ?? 0 })}
                      </span>
                    {/if}
                  </button>
                  <button class="block-x" title={tr("Снять блок")} onclick={(e) => { e.stopPropagation(); unschedule(t.id); }}>✕</button>
                  <div class="resize-handle" role="presentation" onmousedown={(e) => startResize(e, t)}></div>
                </div>
              {/each}

              {#each routinesByDay.get(d.key) ?? [] as rb, ri (ri)}
                <div
                  class="block routine"
                  role="presentation"
                  style="top:{(rb.start_mins / 60) * HOUR_H}px; height:{Math.max((rb.duration_mins / 60) * HOUR_H, 18)}px;"
                  title="{rb.title}"
                >
                  <div class="block-body">
                    <span class="block-time">{hhmmFromMins(rb.start_mins)}–{hhmmFromMins(rb.start_mins + rb.duration_mins)}</span>
                    <span class="block-title">{rb.title}</span>
                  </div>
                </div>
              {/each}

              {#each proposedByDay.get(d.key) ?? [] as b (b.id)}
                <div
                  class="block ghost"
                  role="listitem"
                  style="top:{ghostTop(b)}px; height:{Math.max((b.mins / 60) * HOUR_H, 18)}px;"
                  title="{tr('Предложение ИИ')}: {ghostLabel(b)} · {b.title}"
                >
                  <div class="block-body">
                    <span class="block-time">{ghostLabel(b)}</span>
                    <span class="block-title">{b.title}</span>
                  </div>
                </div>
              {/each}
            </div>
          {/each}
        </div>
      </div>
    </div>

    <aside class="backlog card">
      <div class="section-title" style="margin-bottom:8px;">{tr("Бэклог")}</div>

      {#if aiEnabled}
        {#if proposed}
          <div class="plan-bar">
            <span class="plan-hint">{tr("ИИ предложил {n} блок(а) — пунктиром в сетке", { n: proposed.length })}</span>
            <div class="plan-actions">
              <button class="btn-primary btn-sm" onclick={applyPlan}>{tr("Применить")}</button>
              <button class="btn-ghost btn-sm" onclick={() => proposed = null}>{tr("Отмена")}</button>
            </div>
          </div>
        {:else}
          <button class="btn-sm plan-btn" onclick={planDay} disabled={planning || backlog.length === 0}
            title={tr("ИИ разложит важные задачи из бэклога по свободному времени сегодня")}>
            {#if planning}{tr("Планирую…")}{:else}<Icon name="zap" size={12} /> {tr("Спланировать день")}{/if}
          </button>
        {/if}
        {#if planError}
          <div class="plan-error">
            {tErr(planError)}
            <button class="btn-icon" onclick={() => planError = null}>✕</button>
          </div>
        {/if}
      {/if}

      {#if backlog.length === 0}
        <p class="muted" style="font-size:12px;margin:0;">{tr("Все активные задачи уже в расписании")}</p>
      {:else}
        {#each backlog as t (t.id)}
          <div
            class="backlog-item"
            draggable="true"
            role="listitem"
            ondragstart={(e) => onBacklogDragStart(e, t)}
            title={tr("Перетащите на день и время")}
          >{t.title}</div>
        {/each}
      {/if}
      <p class="muted" style="font-size:11px;margin:10px 0 0 0;">
        {tr("Перетащите задачу в сетку — блок встанет с шагом {n} мин.", { n: SNAP_MIN })}
        {tr("Нижняя кромка блока тянется мышью.")}
      </p>
    </aside>
  </div>
  {:else}
  <div class="month-grid">
    {#each WEEKDAYS as wd}
      <div class="weekday">{wd}</div>
    {/each}

    {#each grid as cell (cell.key)}
      <div
        class="day card"
        class:today={cell.isToday}
        class:out={!cell.inMonth}
        onclick={() => createFor = cell.key}
        onkeydown={(e) => { if (e.key === "Enter" && e.target === e.currentTarget) createFor = cell.key; }}
        role="button"
        tabindex="0"
        title={tr("Создать задачу на этот день")}
      >
        <div class="day-num" class:today={cell.isToday}>{cell.day}</div>
        <div class="day-tasks">
          {#each cell.tasks.slice(0, MAX_CHIPS) as t (t.id)}
            <button class="task-chip {chipClass(t)}" onclick={(e) => { e.stopPropagation(); openTaskId = t.id; }} title={t.title}>
              {t.title}
            </button>
          {/each}
          {#if cell.tasks.length > MAX_CHIPS}
            <span class="more">{tr("+{n} ещё", { n: cell.tasks.length - MAX_CHIPS })}</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  <p class="muted" style="font-size:12px;margin-top:10px;">
    {tr("Задачи разложены по дате дедлайна. Красные — просроченные, зачёркнутые — выполненные. Клик по задаче открывает её, клик по дню — создаёт задачу с дедлайном на этот день.")}
  </p>
  {/if}
</div>

{#if createFor}
  <TaskModal
    initialDeadline={`${createFor}T09:00`}
    onSave={handleCreate}
    onClose={() => createFor = null}
  />
{/if}

{#if openTaskId}
  <TaskOpener taskId={openTaskId} onClose={() => openTaskId = null} />
{/if}

{#if showRoutinesModal}
  <RoutinesModal onClose={() => showRoutinesModal = false} />
{/if}

<style>
  .page-head {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 12px;
  }

  .month-label {
    min-width: 130px;
    text-align: center;
    font-weight: 600;
    font-size: 13px;
  }

  .month-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px;
  }

  .weekday {
    text-align: center;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: .04em;
    color: var(--text-secondary);
    padding: 2px 0 4px;
  }

  .day {
    min-height: 86px;
    padding: 4px;
    min-width: 0;
    cursor: pointer;
  }

  .day:hover {
    background: var(--bg-hover);
  }

  .day.today {
    border-color: var(--accent);
  }

  .day.out {
    opacity: 0.45;
  }

  .day-num {
    font-size: 11px;
    color: var(--text-secondary);
    margin-bottom: 3px;
    padding-left: 2px;
  }

  .day-num.today {
    color: var(--accent);
    font-weight: 700;
  }

  .day-tasks {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .task-chip {
    display: block;
    width: 100%;
    text-align: left;
    font-size: 11px;
    padding: 2px 5px;
    border-radius: 4px;
    border: none;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
  }

  .task-chip:hover {
    background: color-mix(in srgb, var(--accent) 24%, transparent);
  }

  .task-chip.overdue {
    background: color-mix(in srgb, var(--danger) 14%, transparent);
    color: var(--danger);
  }

  .task-chip.overdue:hover {
    background: color-mix(in srgb, var(--danger) 24%, transparent);
  }

  .task-chip.done {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text-secondary);
    text-decoration: line-through;
    padding: 1px 4px;
  }

  .more {
    font-size: 11px;
    color: var(--text-secondary);
    padding-left: 5px;
  }

  /* ===== Week ===== */
  /* The look comes from the shared .seg in app.css; only the gap from the heading is set here. */
  .mode-toggle {
    margin-left: 12px;
  }

  .week-layout {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .week-main {
    flex: 1;
    min-width: 0;
    padding: 0;
    overflow: hidden;
  }

  .week-head {
    display: grid;
    grid-template-columns: 48px repeat(7, 1fr);
    border-bottom: 1px solid var(--border);
  }

  .week-day-head {
    text-align: center;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: .04em;
    color: var(--text-secondary);
    padding: 6px 0;
  }

  .week-day-head.today {
    color: var(--accent);
    font-weight: 700;
  }

  .week-scroll {
    max-height: calc(100vh - 190px);
    overflow-y: auto;
  }

  .week-grid {
    display: grid;
    grid-template-columns: 48px repeat(7, 1fr);
  }

  .hour-gutter {
    border-right: 1px solid var(--border);
  }

  .hour-mark {
    font-size: 10px;
    color: var(--text-secondary);
    text-align: right;
    padding-right: 6px;
    box-sizing: border-box;
    border-top: 1px solid transparent;
    transform: translateY(-6px);
  }

  .week-col {
    position: relative;
    border-right: 1px solid var(--border);
    background-image: linear-gradient(to bottom, var(--border) 1px, transparent 1px);
  }

  .week-col:last-child {
    border-right: none;
  }

  .week-col.today {
    background-color: color-mix(in srgb, var(--accent) 4%, transparent);
  }

  .block {
    position: absolute;
    left: 2px;
    right: 2px;
    background: color-mix(in srgb, var(--accent) 16%, var(--bg-primary));
    border-left: 3px solid var(--accent);
    border-radius: 4px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    cursor: grab;
    z-index: 1;
  }

  .block:hover {
    background: color-mix(in srgb, var(--accent) 26%, var(--bg-primary));
    z-index: 2;
  }

  /* An AI proposal: a translucent dashed "ghost" until it is confirmed */
  /* A routine: a translucent block with no interactivity */
  .block.routine {
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-primary));
    border: 1px dashed var(--accent);
    border-left-width: 3px;
    cursor: default;
    opacity: 0.6;
    pointer-events: none;
    z-index: 0;
  }

  .block.routine .block-body {
    padding-right: 5px;
    cursor: default;
  }

  .block.routine .block-time {
    color: var(--text-secondary);
  }

  .block.ghost {
    background: color-mix(in srgb, var(--accent) 7%, var(--bg-primary));
    border: 1.5px dashed var(--accent);
    border-left-width: 3px;
    cursor: default;
    opacity: 0.85;
  }

  .block.ghost .block-body {
    padding-right: 5px;
    cursor: default;
  }

  .plan-btn {
    width: 100%;
    margin-bottom: 10px;
  }

  .plan-bar {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    margin-bottom: 10px;
    border: 1.5px dashed var(--accent);
    border-radius: 6px;
  }

  .plan-hint {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .plan-actions {
    display: flex;
    gap: 6px;
  }

  .plan-error {
    display: flex;
    align-items: flex-start;
    gap: 4px;
    font-size: 11px;
    color: var(--danger);
    margin-bottom: 10px;
  }

  .block-body {
    flex: 1;
    min-height: 0;
    background: transparent;
    border: none;
    padding: 2px 16px 2px 5px;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 1px;
    overflow: hidden;
    cursor: inherit;
    color: inherit;
  }

  .block-time {
    font-size: 10px;
    color: var(--accent);
    font-weight: 600;
    white-space: nowrap;
  }

  .block-title {
    font-size: 11px;
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Idle time is muted because it states a fact rather than warning about
     one. In red it would read as a reproach for stepping away from the
     computer. */
  .block-idle {
    font-size: 10px;
    opacity: 0.75;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .block-x {
    position: absolute;
    top: 1px;
    right: 1px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-size: 10px;
    padding: 1px 4px;
    opacity: 0;
  }

  .block:hover .block-x {
    opacity: 1;
  }

  .resize-handle {
    height: 6px;
    cursor: ns-resize;
    flex-shrink: 0;
  }

  .backlog {
    width: 200px;
    flex-shrink: 0;
    padding: 12px;
  }

  .backlog-item {
    font-size: 12px;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    margin-bottom: 5px;
    cursor: grab;
    background: var(--bg-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .backlog-item:active {
    cursor: grabbing;
  }
</style>
