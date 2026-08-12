<script lang="ts">
  import { onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { api } from "../lib/api/tauri";
  import { projectStore } from "../lib/stores/projects.svelte";
  import { categoryStore } from "../lib/stores/categories.svelte";
  import type { AppSettings, DayCompletion } from "../lib/types";
  import TaskOpener from "../lib/components/TaskOpener.svelte";
  import { t, tErr, i18n } from "../lib/i18n.svelte";
  import { localDateKey, pad2, localeTag } from "../lib/datetime";
  import { formatMinutes } from "../lib/duration";
  import { clampTipX } from "../lib/menuPos";
  import { loadUiState, saveUiState } from "../lib/uistate";

  // A task opens right on the dashboard, without leaving for the Tasks screen
  let openTaskId = $state<string | null>(null);

  interface ActivityDay {
    date: string;
    minutes: number;
  }

  interface TaskCompletion {
    date: string;
    completed: number;
  }

  interface CategoryCount {
    category: string;
    count: number;
  }

  interface ActiveIdleRatio {
    today_active: number;
    today_idle: number;
    week_active: number;
    week_idle: number;
  }

  interface PomodoroStats {
    today: number;
    week: number;
    task_streak: number;
    pomodoro_streak: number;
  }

  interface InsightPayload {
    result: string | null;
    error: string | null;
  }

  interface SummaryPayload {
    kind: "day" | "week";
    result: string | null;
    error: string | null;
  }

  let activityDays: ActivityDay[] = $state([]);
  let taskCompletions: TaskCompletion[] = $state([]);
  let categories: CategoryCount[] = $state([]);
  let ratio: ActiveIdleRatio | null = $state(null);
  let settings: AppSettings | null = $state(null);
  let error: string | null = $state(null);
  let pomodoroStats: PomodoroStats | null = $state(null);

  // Applications: the top by active minutes plus time per category (rules from Settings)
  let appUsage: { app: string; minutes: number }[] = $state([]);
  let appCategories: { category: string; minutes: number }[] = $state([]);
  let appPeriod: 1 | 7 = $state(1);
  let domainUsage: { domain: string; minutes: number }[] = $state([]);

  async function loadAppUsage(days: 1 | 7) {
    appPeriod = days;
    saveUiState({ dashboardAppPeriod: days });
    try {
      appUsage = await api.getAppUsage(days);
      appCategories = await api.getAppCategoryTime(days);
      // Empty while track_domains is off — a normal state, the sites block is
      // simply not shown.
      domainUsage = await api.getDomainUsage(days);
    } catch {
      appUsage = [];
      appCategories = [];
      domainUsage = [];
    }
  }

  let insightText: string | null = $state(null);
  let insightError: string | null = $state(null);
  let insightPending = $state(false);

  let summaryText: string | null = $state(null);
  let summaryError: string | null = $state(null);
  let summaryPending: "day" | "week" | null = $state(null);
  let summaryKind: "day" | "week" | null = $state(null);

  // Labels for APPLICATION categories (the glob tracking rules, a fixed set). Task
  // categories are user-defined and come from categoryStore. This is a FIXED set in
  // the code rather than rows from the DB (unlike task categories, migration 0015),
  // which is why it is translated, keyed by the category id.
  const CATEGORY_LABELS: Record<string, string> = $derived({
    Work: t("Работа"),
    Study: t("Учёба"),
    Home: t("Дом"),
    Health: t("Здоровье"),
    Other: t("Другое"),
  });

  const donutData = $derived.by(() => {
    // The order and colours come from the categories table; legacy values (the
    // category was deleted but tasks remain in history) go grey at the end.
    const known = categoryStore.categories.map(c => ({
      category: c.id,
      label: categoryStore.name(c.id),
      color: c.color,
      count: categories.find(x => x.category === c.id)?.count ?? 0,
    }));
    const knownIds = new Set(categoryStore.categories.map(c => c.id));
    const legacy = categories
      .filter(x => !knownIds.has(x.category))
      .map(x => ({ category: x.category, label: x.category, color: "#888888", count: x.count }));
    return [...known, ...legacy].filter(d => d.count > 0);
  });
  const donutTotal = $derived(donutData.reduce((s, d) => s + d.count, 0));

  const R = 45;
  const CIRC = 2 * Math.PI * R;
  const SEG_GAP = 2; // px of gap between segments

  // The donut's segments: arc length minus the gap, with a cumulative offset.
  const donutSegments = $derived.by(() => {
    const gap = donutData.length > 1 ? SEG_GAP : 0;
    let start = 0;
    return donutData.map(d => {
      const arc = (d.count / donutTotal) * CIRC;
      const seg = {
        ...d,
        dash: Math.max(arc - gap, 0.1),
        offset: -(start + gap / 2),
        percent: Math.round((d.count / donutTotal) * 100),
      };
      start += arc;
      return seg;
    });
  });

  function pct(active: number, idle: number): number | null {
    const total = active + idle;
    return total > 0 ? Math.round((active / total) * 100) : null;
  }

  // --- The year in squares: completed tasks by local day ---
  const YEAR_DAYS = 365;

  // "2026-08" from "2026-08-10". Grouping by the calendar month rather than by
  // grid column on purpose: a column is a week, and a week straddles the month
  // boundary at both ends, so highlighting whole columns would light up days
  // belonging to the neighbouring month.
  function monthKey(date: string): string {
    return date.slice(0, 7);
  }

  // The month under the pointer, highlighted across the grid. Hovering the label
  // is not enough on its own — it is a few characters wide, while the month it
  // names covers about thirty squares — so the squares set it too.
  let hoveredMonth: string | null = $state(null);


  const calendar = $derived.by(() => {
    const byDate = new Map(taskCompletions.map(c => [c.date, c.completed]));
    const today = new Date();
    const days: { date: string; count: number }[] = [];
    for (let i = YEAR_DAYS - 1; i >= 0; i--) {
      const d = new Date(today.getFullYear(), today.getMonth(), today.getDate() - i);
      const key = localDateKey(d);
      days.push({ date: key, count: byDate.get(key) ?? 0 });
    }
    // Empty cells at the start so the week columns begin on Monday
    const first = new Date(today.getFullYear(), today.getMonth(), today.getDate() - (YEAR_DAYS - 1));
    const lead = (first.getDay() + 6) % 7;
    // Derived rather than a hardcoded 53 so the grid cannot spill into implicit
    // columns, which are sized by a different rule than the explicit ones. For 365
    // days this is always 53 — 365 = 52*7 + 1, so even a 6-cell lead still fits the
    // last column — but that is a property of YEAR_DAYS, not something the layout
    // should assume.
    const cols = Math.ceil((lead + days.length) / 7);

    // A label for each month, in the column holding its first day. The month name
    // comes from Intl rather than the dictionary: it follows the chosen language
    // without 12 entries per locale. (The weekday labels beside the grid go the
    // other way and sit in the dictionary — three letters each, and they have to
    // be abbreviated to a fixed width that Intl does not guarantee.)
    //
    // A month whose label would crowd the previous one is skipped: the year starts
    // mid-month, so its first label can describe only two or three squares.
    const MIN_COLS_BETWEEN = 3;
    const fmt = new Intl.DateTimeFormat(localeTag(i18n.lang), { month: "short" });
    const months: { col: number; label: string; key: string }[] = [];
    for (let i = 0; i < days.length; i++) {
      const d = new Date(days[i].date + "T00:00:00");
      if (d.getDate() !== 1) continue;
      const col = Math.floor((lead + i) / 7);
      const prev = months[months.length - 1];
      if (prev && col - prev.col < MIN_COLS_BETWEEN) continue;
      months.push({ col, label: fmt.format(d), key: monthKey(days[i].date) });
    }
    return { lead, days, cols, months };
  });

  const calMax = $derived(Math.max(1, ...taskCompletions.map(c => c.completed)));
  const CAL_MIX = [0, 25, 45, 70, 95]; // accent percentages per level

  // The three counters above the grid. All of them come from taskCompletions,
  // which is already loaded for the heatmap — the Dashboard deliberately does not
  // pull in taskStore, which would fetch the whole task list for three numbers.
  const tiles = $derived.by(() => {
    const byDate = new Map(taskCompletions.map(c => [c.date, c.completed]));
    const now = new Date();
    const today = byDate.get(localDateKey(now)) ?? 0;

    let week = 0;
    for (let i = 0; i < 7; i++) {
      const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() - i);
      week += byDate.get(localDateKey(d)) ?? 0;
    }

    const best = Math.max(0, ...taskCompletions.map(c => c.completed));
    return { today, week, best };
  });

  function calLevel(count: number): number {
    if (count <= 0) return 0;
    const r = count / calMax;
    return r > 0.75 ? 4 : r > 0.5 ? 3 : r > 0.25 ? 2 : 1;
  }

  // The locale follows the chosen language rather than a hardcoded "ru-RU": in the
  // calendar tooltip the date used to stay Russian. The same approach as in
  // Notes.svelte.
  function fmtDay(date: string): string {
    return new Date(date + "T00:00:00")
      .toLocaleDateString(localeTag(i18n.lang), { day: "numeric", month: "short" });
  }

  // The tooltip (hover, a quick preview) and the popup (click, with navigation to a
  // task): the day's list of completed tasks is loaded lazily and cached by date.
  let calTip: { date: string; count: number; completions: DayCompletion[]; x: number; y: number } | null = $state(null);
  let calPopup: { date: string; count: number; completions: DayCompletion[] } | null = $state(null);
  const dayCompletionsCache = new Map<string, DayCompletion[]>();

  async function loadDayCompletions(day: { date: string; count: number }): Promise<DayCompletion[]> {
    if (day.count === 0) return [];
    const cached = dayCompletionsCache.get(day.date);
    if (cached) return cached;
    const completions = await api.getCompletionsForDay(day.date).catch(() => []);
    dayCompletionsCache.set(day.date, completions);
    return completions;
  }

  // The tooltip's width, mirroring max-width in .cal-tip. Needed here because the
  // clamp happens before the element exists, so it cannot be measured.
  const CAL_TIP_W = 280;
  // An upper bound for the flip decision, mirroring max-height in .cal-tip. The
  // real height is unknown before the element renders, and a day with many tasks
  // is the case that would otherwise open downwards off the screen.
  const CAL_TIP_MAX_H = 180;
  let calWrapEl: HTMLDivElement | undefined = $state();

  async function showCalTip(e: MouseEvent, day: { date: string; count: number }) {
    const cell = e.currentTarget as HTMLElement;
    // Viewport coordinates: the tooltip is position:fixed at the top level, not
    // inside .cal-wrap, so it is clamped against the window rather than the panel.
    // A hardcoded ceiling of 640px used to park it mid-panel for every cell to the
    // right of that.
    const r = cell.getBoundingClientRect();
    const x = clampTipX(r.left, CAL_TIP_W, window.innerWidth);
    // Below the square by default, above it when there is no room underneath —
    // the year block sits low on the dashboard, so the last rows are near the
    // bottom of the window.
    const below = r.bottom + 8;
    const y = below + CAL_TIP_MAX_H > window.innerHeight ? r.top - CAL_TIP_MAX_H - 8 : below;
    const completions = await loadDayCompletions(day);
    calTip = { date: day.date, count: day.count, completions, x, y: Math.max(8, y) };
  }

  async function openCalPopup(day: { date: string; count: number }) {
    calTip = null;
    const completions = await loadDayCompletions(day);
    calPopup = { date: day.date, count: day.count, completions };
  }

  function openTaskFromPopup(id: string) {
    calPopup = null;
    openTaskId = id;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && calPopup) calPopup = null;
  }

  // --- The "hour x weekday" heatmap ---
  // The activity list opens on the last few days. Thirty rows of a bar chart is a
  // wall that pushes everything below it off the screen, and the recent days are
  // the ones actually read.
  const ACTIVITY_COLLAPSED = 5;
  let activityExpanded = $state(false);

  let hourly: { weekday: number; hour: number; minutes: number }[] = $state([]);
  const HOURS = Array.from({ length: 24 }, (_, i) => i);
  const WEEKDAY_LABELS = $derived([t("Пн"), t("Вт"), t("Ср"), t("Чт"), t("Пт"), t("Сб"), t("Вс")]);
  const heatMax = $derived(Math.max(1, ...hourly.map(c => c.minutes)));

  // row 0 = Monday; in the data weekday follows strftime('%w'), where 0 = Sunday
  function heatMinutes(row: number, hour: number): number {
    const w = (row + 1) % 7;
    return hourly.find(c => c.weekday === w && c.hour === hour)?.minutes ?? 0;
  }

  function heatStyle(mins: number): string {
    if (mins <= 0) return "";
    const p = Math.round(10 + (mins / heatMax) * 85);
    return `background: color-mix(in srgb, var(--accent) ${p}%, transparent);`;
  }

  onMount(() => {
    const unlisteners: UnlistenFn[] = [];

    (async () => {
      try {
        activityDays = await api.getActivityByDay();
        taskCompletions = await api.getTaskCompletionsByDay();
        categories = await api.getCategoryDistribution();
        ratio = await api.getActiveIdleRatio();
        settings = await api.getSettings();
        // The chosen period survives a restart (v0.9.79): loadAppUsage both fetches
        // and sets appPeriod, so the saved value is applied here rather than
        // assigned separately and then overwritten by a hardcoded 1.
        await loadAppUsage(loadUiState().dashboardAppPeriod === 7 ? 7 : 1);
        await projectStore.load(); // fresh goal progress for the period
        await loadProjectTime();
        await categoryStore.load(); // task category names and colours for the donut
        hourly = await api.getHourlyActivity(56); // heatmap: the last 8 weeks
        pomodoroStats = await api.getPomodoroStats();

      } catch (e) {
        error = String(e);
      }

      unlisteners.push(await listen<InsightPayload>("dashboard-insight", (event) => {
        insightPending = false;
        insightText = event.payload.result;
        insightError = event.payload.error;
      }));

      unlisteners.push(await listen<SummaryPayload>("period-summary", (event) => {
        summaryPending = null;
        summaryKind = event.payload.kind;
        summaryText = event.payload.result;
        summaryError = event.payload.error;
      }));
    })();

    return () => unlisteners.forEach(u => u());
  });

  async function refreshSummary(kind: "day" | "week") {
    summaryPending = kind;
    summaryError = null;
    try {
      await (kind === "day" ? api.summarizeDay() : api.summarizeWeek());
    } catch (e) {
      summaryPending = null;
      summaryError = String(e);
    }
  }

  async function refreshInsight() {
    insightPending = true;
    insightError = null;
    try {
      await api.dashboardInsight();
    } catch (e) {
      insightPending = false;
      insightError = String(e);
    }
  }

  function maxMinutes(days: ActivityDay[]) {
    return Math.max(...days.map(d => d.minutes), 1);
  }

  // Project goals: active projects with a goal set (tasks and/or minutes)
  const goalProjects = $derived(
    projectStore.active.filter(p => p.goal_tasks != null || p.goal_mins != null)
  );

  const goalPct = (done: number, target: number) =>
    Math.min(100, Math.round((done / Math.max(target, 1)) * 100));

  let projectTime: { name: string; mins: number }[] = $state([]);
  async function loadProjectTime() {
    const now = new Date();
    const weekAgo = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 7);
    const result: { name: string; mins: number }[] = [];
    for (const p of projectStore.active) {
      try {
        const secs = await api.getProjectSeconds(p.id, weekAgo.toISOString());
        if (secs >= 60) result.push({ name: p.name, mins: Math.round(secs / 60) });
      } catch {}
    }
    projectTime = result.sort((a, b) => b.mins - a.mins);
  }
</script>

<div class="dash">
  <h2 class="page-title" style="margin-bottom:14px;">{t("Дашборд")}</h2>

  {#if error}
    <div class="alert">{error}</div>
  {/if}

  <div class="tiles">
    <div class="tile">
      <div class="tile-n">{tiles.today}</div>
      <div class="tile-l">{t("Сегодня")}</div>
    </div>
    <div class="tile">
      <div class="tile-n">{tiles.week}</div>
      <div class="tile-l">{t("За неделю")}</div>
    </div>
    <div class="tile">
      <div class="tile-n">{tiles.best}</div>
      <div class="tile-l">{t("Лучший день")}</div>
    </div>
  </div>

  <div class="grid">
    <!-- The donut by category -->
    <section class="card panel">
      <h3 class="section-title">{t("Выполнено по категориям")}</h3>

      {#if donutData.length === 0}
        <div class="empty">{t("Нет выполненных задач")}</div>
      {:else}
        <div class="donut-row">
          <svg viewBox="0 0 120 120" width="132" height="132" role="img" aria-label={t("Выполненные задачи по категориям")}>
            <g transform="rotate(-90 60 60)">
              {#each donutSegments as seg (seg.category)}
                <circle
                  cx="60" cy="60" r={R}
                  fill="none"
                  stroke={seg.color}
                  stroke-width="16"
                  stroke-dasharray="{seg.dash} {CIRC - seg.dash}"
                  stroke-dashoffset={seg.offset}
                >
                  <title>{seg.label}: {seg.count} ({seg.percent}%)</title>
                </circle>
              {/each}
            </g>
            <text x="60" y="57" text-anchor="middle" class="donut-total">{donutTotal}</text>
            <text x="60" y="74" text-anchor="middle" class="donut-caption">{t("всего")}</text>
          </svg>

          <ul class="legend">
            {#each donutSegments as seg (seg.category)}
              <li>
                <span class="swatch" style="background:{seg.color};"></span>
                <span>{seg.label}</span>
                <span class="muted">{seg.count} · {seg.percent}%</span>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </section>

    <!-- Active versus idle -->
    <section class="card panel">
      <h3 class="section-title">{t("Активное время")}</h3>

      {#if ratio}
        {#each [
          { label: t("Сегодня"), value: pct(ratio.today_active, ratio.today_idle), active: ratio.today_active },
          { label: t("Неделя"), value: pct(ratio.week_active, ratio.week_idle), active: ratio.week_active },
        ] as row (row.label)}
          <div class="ratio-row">
            <div class="ratio-head">
              <span>{row.label}</span>
              <span class="muted">
                {#if row.value === null}
                  {t("нет данных")}
                {:else}
                  {t("{pct}% актив · {dur}", { pct: row.value, dur: formatMinutes(row.active / 60, t) })}
                {/if}
              </span>
            </div>
            <div class="track">
              <div class="fill" style="width:{row.value ?? 0}%;"></div>
            </div>
          </div>
        {/each}
      {:else}
        <div class="empty">{t("Нет данных")}</div>
      {/if}
    </section>

    <section class="card panel">
      <h3 class="section-title">{t("ИИ-инсайт")}</h3>

      {#if settings && settings.ai_provider === "none"}
        <p class="muted" style="margin:0;">{t("ИИ отключён — включите провайдера в Настройках, чтобы получать инсайты.")}</p>
      {:else}
        <div class="ai-row">
          <button onclick={refreshInsight} disabled={insightPending}>
            {insightPending ? t("Думаю…") : t("Обновить")}
          </button>
          <div class="ai-text">
            {#if insightError}
              <span style="color:var(--danger);">{tErr(insightError)}</span>
            {:else if insightText}
              {insightText}
            {:else if !insightPending}
              <span class="muted">{t("Нажмите «Обновить», чтобы получить короткий разбор вашей продуктивности.")}</span>
            {/if}
          </div>
        </div>
      {/if}
    </section>

    <!-- Pomodoro: statistics and streaks -->
    {#if pomodoroStats}
      <section class="card panel">
        <h3 class="section-title">{t("Помодоро")}</h3>
        <!-- Its own rows rather than .goal-row: that layout is built around a
             .track bar taking flex:1 between the label and the value. With no bar
             to absorb the slack, a fixed 48px label and a 52px value left the
             whole panel empty to the right. Here the label and the value are
             pushed to opposite edges. -->
        <dl class="stat-rows">
          <div class="stat-row">
            <dt>{t("сегодня")}</dt>
            <dd>{pomodoroStats.today}</dd>
          </div>
          <div class="stat-row">
            <dt>{t("за неделю")}</dt>
            <dd>{pomodoroStats.week}</dd>
          </div>
          <div class="stat-row">
            <dt>{t("стрик задач")}</dt>
            <dd>{t("{n} дн.", { n: pomodoroStats.task_streak })}</dd>
          </div>
          <div class="stat-row">
            <dt>{t("стрик помидоров")}</dt>
            <dd>{t("{n} дн.", { n: pomodoroStats.pomodoro_streak })}</dd>
          </div>
        </dl>
      </section>
    {/if}

    <!-- The day/week digest -->
    <section class="card panel">
      <h3 class="section-title">{t("Резюме")}</h3>

      {#if settings && settings.ai_provider === "none"}
        <p class="muted" style="margin:0;">{t("ИИ отключён — резюме недоступно.")}</p>
      {:else}
        <div class="ai-row">
          <div class="btn-group">
            <button onclick={() => refreshSummary("day")} disabled={summaryPending !== null}>
              {summaryPending === "day" ? t("Думаю…") : t("День")}
            </button>
            <button onclick={() => refreshSummary("week")} disabled={summaryPending !== null}>
              {summaryPending === "week" ? t("Думаю…") : t("Неделя")}
            </button>
          </div>
          <div class="ai-text">
            {#if summaryError}
              <span style="color:var(--danger);">{tErr(summaryError)}</span>
            {:else if summaryText}
              <span class="muted" style="font-size:11px;display:block;margin-bottom:2px;">
                {summaryKind === "day" ? t("За день") : t("За неделю")}
              </span>
              {summaryText}
            {:else if summaryPending === null}
              <span class="muted">{t("Резюме дня или недели: что сделано и сколько времени было активным.")}</span>
            {/if}
          </div>
        </div>
      {/if}
    </section>

    <!-- Project goals (only if any project has one set) -->
    {#if goalProjects.length > 0}
      <section class="card panel">
        <h3 class="section-title">{t("Цели проектов")}</h3>
        <ul class="goals">
          {#each goalProjects as p (p.id)}
            <li class="goal-item">
              <div class="goal-head">
                <span class="goal-name">{p.name}</span>
                <span class="muted">{p.goal_period === "month" ? t("месяц") : t("неделя")}</span>
              </div>
              {#if p.goal_tasks != null}
                <div class="goal-row">
                  <span class="goal-metric">{t("задачи")}</span>
                  <div class="track">
                    <div class="fill" class:done={p.goal_done_tasks >= p.goal_tasks}
                      style="width:{goalPct(p.goal_done_tasks, p.goal_tasks)}%;"></div>
                  </div>
                  <span class="goal-val muted">{p.goal_done_tasks}/{p.goal_tasks}</span>
                </div>
              {/if}
              {#if p.goal_mins != null}
                <div class="goal-row">
                  <span class="goal-metric">{t("минуты")}</span>
                  <div class="track">
                    <div class="fill" class:done={p.goal_done_mins >= p.goal_mins}
                      style="width:{goalPct(p.goal_done_mins, p.goal_mins)}%;"></div>
                  </div>
                  <span class="goal-val muted">{p.goal_done_mins}/{p.goal_mins}</span>
                </div>
              {/if}
            </li>
          {/each}
        </ul>
        <p class="muted" style="font-size:11px;margin:8px 0 0 0;">
          {t("Минуты — по трекингу задач проекта за период.")}
        </p>
      </section>
    {/if}

    {#if projectTime.length > 0}
      <section class="card panel">
        <h3 class="section-title">{t("Время по проектам (7 дней)")}</h3>
        <ul class="goals">
          {#each projectTime as pt}
            <li class="goal-item">
              <div class="goal-head">
                <span class="goal-name">{pt.name}</span>
              </div>
              <div class="goal-row">
                <span class="goal-val muted">{formatMinutes(pt.mins, t)}</span>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <!-- Sites: a separate card rather than a block inside "Applications".
         Otherwise the per-site breakdown would depend on whether anything was
         recorded per application — two independent data sources must not hide
         one another. The card is absent entirely while domain tracking is off:
         an empty "Sites" heading would read as a breakage rather than as a
         disabled feature. -->
    {#if domainUsage.length > 0}
      {@const maxDomain = Math.max(...domainUsage.map(d => d.minutes), 1)}
      <section class="card panel wide">
        <h3 class="section-title">{t("Сайты")}</h3>
        <div class="rows">
          {#each domainUsage as d (d.domain)}
            <div class="bar-row">
              <span class="bar-date" title={d.domain}>{d.domain}</span>
              <div class="track tall">
                <div class="fill" style="width:{Math.round((d.minutes / maxDomain) * 100)}%;"></div>
              </div>
              <span class="bar-val">{formatMinutes(d.minutes, t)}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Applications (only if the window provider recorded anything) -->
    {#if appUsage.length > 0}
      {@const maxApp = Math.max(...appUsage.map(a => a.minutes), 1)}
      <section class="card panel wide">
        <div class="apps-head">
          <h3 class="section-title" style="margin:0;">{t("Приложения")}</h3>
          <div class="seg">
            <button class:active={appPeriod === 1} onclick={() => loadAppUsage(1)}>{t("Сегодня")}</button>
            <button class:active={appPeriod === 7} onclick={() => loadAppUsage(7)}>{t("Неделя")}</button>
          </div>
        </div>

        <div class="apps-cols">
          <div class="rows" style="flex:1;min-width:0;">
            {#each appUsage as a (a.app)}
              <div class="bar-row">
                <span class="bar-date" title={a.app}>{a.app}</span>
                <div class="track tall">
                  <div class="fill" style="width:{Math.round((a.minutes / maxApp) * 100)}%;"></div>
                </div>
                <span class="bar-val">{formatMinutes(a.minutes, t)}</span>
              </div>
            {/each}
          </div>

          <ul class="legend">
            {#each appCategories as c (c.category)}
              <li>
                <span class="swatch" style="background:var(--cat-{c.category.toLowerCase()});"></span>
                <span>{CATEGORY_LABELS[c.category] ?? c.category}</span>
                <span class="muted">{formatMinutes(c.minutes, t)}</span>
              </li>
            {/each}
          </ul>
        </div>

        <p class="muted" style="font-size:11px;margin:8px 0 0 0;">
          {t("Категории — по правилам «класс окна → категория» в Настройках → Мониторинг.")}
        </p>
      </section>
    {/if}

    <!-- Activity by day -->
    <section class="card panel wide">
      <h3 class="section-title">{t("Активность по дням (мин)")}</h3>

      {#if activityDays.length === 0}
        <div class="empty">{t("Нет данных")}</div>
      {:else}
        {@const shown = activityDays.slice(-30)}
        <!-- The scale stays tied to the full 30 days, not to what is on screen:
             if it followed the collapsed slice, expanding would rescale every bar
             already visible and the same day would look busier collapsed than
             expanded. -->
        {@const max = maxMinutes(shown)}
        <div class="rows">
          {#each activityExpanded ? shown : shown.slice(-ACTIVITY_COLLAPSED) as day (day.date)}
            <div class="bar-row">
              <span class="bar-date">{day.date}</span>
              <div class="track tall">
                <div class="fill" style="width:{Math.round((day.minutes / max) * 100)}%;"></div>
              </div>
              <span class="bar-val">{formatMinutes(day.minutes, t)}</span>
            </div>
          {/each}
        </div>
        {#if shown.length > ACTIVITY_COLLAPSED}
          <button
            class="btn-ghost btn-sm activity-more"
            aria-expanded={activityExpanded}
            onclick={() => activityExpanded = !activityExpanded}
          >
            {activityExpanded
              ? t("Свернуть список")
              : t("Показать все ({n})", { n: shown.length })}
          </button>
        {/if}
      {/if}
    </section>

    <!-- The year in squares: completed tasks -->
    <section class="card panel wide">
      <h3 class="section-title">{t("Выполненные задачи за год")}</h3>

      {#if taskCompletions.length === 0}
        <div class="empty">{t("Нет данных")}</div>
      {:else}
        <div
          class="cal-wrap"
          bind:this={calWrapEl}
          onmouseleave={() => { calTip = null; hoveredMonth = null; }}
          role="presentation"
        >
          <!-- Month labels. Each sits in the column where its month starts, so the
               label lines up with the first square of that month. aria-hidden: the
               squares already carry a full date in aria-label, and a screen reader
               reading a row of month names on its own would only add noise. -->
          <div class="cal-months" style="--cal-cols: {calendar.cols}" aria-hidden="true">
            {#each calendar.months as m (m.col)}
              <!-- +2, not +1: column 1 is the weekday-label gutter.
                   Hovering the name outlines that month across the grid. It stays a
                   span rather than becoming a button: this row is aria-hidden, and a
                   focusable control inside hidden content is reachable by keyboard
                   while being unreadable by a screen reader. The squares below are
                   the accessible path — they are real buttons, each with its date. -->
              <span
                class="cal-month"
                class:on={hoveredMonth === m.key}
                style="grid-column: {m.col + 2}"
                onmouseenter={() => hoveredMonth = m.key}
                role="presentation"
              >{m.label}</span>
            {/each}
          </div>
          <div class="cal-grid" style="--cal-cols: {calendar.cols}">
            <!-- Weekday labels, one gutter column before the year. Only every other
                 day is named: at this cell size seven stacked labels collide, and
                 Mon/Wed/Fri is enough to count off the rows in between. The blanks
                 are still rendered so each label keeps its own grid row.
                 aria-hidden, like the month row: every square already carries its
                 full date in aria-label, so read aloud these would only be noise. -->
            {#each WEEKDAY_LABELS as label, row (label)}
              <span class="cal-wd" style="grid-area: {row + 1} / 1" aria-hidden="true"
                >{row % 2 === 0 ? label : ""}</span>
            {/each}
            {#each Array(calendar.lead) as _, i (i)}
              <span class="cal-cell lead"></span>
            {/each}
            {#each calendar.days as day (day.date)}
              <button
                type="button"
                class="cal-cell"
                class:on={hoveredMonth === monthKey(day.date)}
                data-date={day.date}
                data-count={day.count}
                data-month={monthKey(day.date)}
                style={day.count > 0
                  ? `background: color-mix(in srgb, var(--accent) ${CAL_MIX[calLevel(day.count)]}%, transparent);`
                  : ""}
                onmouseenter={(e) => { hoveredMonth = monthKey(day.date); showCalTip(e, day); }}
                onfocus={() => hoveredMonth = monthKey(day.date)}
                onclick={() => openCalPopup(day)}
                aria-label="{fmtDay(day.date)}: {day.count}"
              ></button>
            {/each}
          </div>
        </div>
      {/if}
    </section>

    <!-- Heatmap: the hours when work actually happens -->
    <section class="card panel wide">
      <h3 class="section-title">{t("Активность по часам (8 недель)")}</h3>

      {#if hourly.length === 0}
        <div class="empty">{t("Нет данных")}</div>
      {:else}
        <div class="heat">
          {#each WEEKDAY_LABELS as label, row (label)}
            <span class="heat-label">{label}</span>
            {#each HOURS as h (h)}
              {@const mins = heatMinutes(row, h)}
              <span
                class="heat-cell"
                style={heatStyle(mins)}
                title="{label} {pad2(h)}:00 — {formatMinutes(mins, t)}"
              ></span>
            {/each}
          {/each}
          <span class="heat-label"></span>
          {#each HOURS as h (h)}
            <span class="heat-hour">{h % 6 === 0 ? h : ""}</span>
          {/each}
        </div>
      {/if}
    </section>
  </div>
</div>

<svelte:window onkeydown={handleKeydown} />

<!-- Outside .cal-wrap on purpose. While it lived inside, a tooltip near the right
     edge stuck out of a box with overflow-x: auto, and the panel grew a scrollbar
     for content nobody scrolls. Here it is fixed to the viewport, so it can hang
     over the panel's edge like the popup below already does. -->
{#if calTip}
  <div class="cal-tip" style="left:{calTip.x}px; top:{calTip.y}px;">
    <div class="cal-tip-head">{fmtDay(calTip.date)} — {calTip.count > 0 ? t("выполнено: {n}", { n: calTip.count }) : t("пусто")}</div>
    {#each calTip.completions as c (c.id)}
      <div class="cal-tip-item">• {c.title}</div>
    {/each}
  </div>
{/if}

{#if calPopup}
  <div role="dialog" aria-modal="true" class="overlay backdrop" onclick={(e) => { if (e.target === e.currentTarget) calPopup = null; }}>
    <div class="modal dialog cal-popup">
      <h2 class="dialog-title">{fmtDay(calPopup.date)} — {calPopup.count > 0 ? t("выполнено: {n}", { n: calPopup.count }) : t("пусто")}</h2>
      {#if calPopup.completions.length === 0}
        <div class="empty">{t("Нет выполненных задач в этот день")}</div>
      {:else}
        <ul class="cal-popup-list">
          {#each calPopup.completions as c (c.id)}
            <li>
              <button type="button" class="cal-popup-item" onclick={() => openTaskFromPopup(c.id)}>{c.title}</button>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="actions">
        <button class="btn-ghost" onclick={() => calPopup = null}>{t("Закрыть")}</button>
      </div>
    </div>
  </div>
{/if}

{#if openTaskId}
  <TaskOpener taskId={openTaskId} onClose={() => openTaskId = null} />
{/if}

<style>
  /* Wider than a text column and centred: the panels here are charts, and a
     chart gains from width where a paragraph loses. */
  .dash {
    max-width: 1200px;
    margin: 0 auto;
  }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin-bottom: 12px;
  }

  .panel {
    padding: 14px 16px;
    min-width: 0;
  }

  .panel.wide {
    grid-column: 1 / -1;
  }

  /* A half-width panel with nothing beside it stretches across instead of leaving
     a hole.
     The case: an odd number of narrow panels followed by a wide one. The wide
     panel cannot share a row, so the last narrow one is left alone with the whole
     right half empty — measured with "Резюме" sitting there down to the next wide
     panel. The count is not fixed either: goals, project time, apps and sites are
     conditional, so which panel lands last depends on the data.

     grid-auto-flow: dense was tried first and does nothing here — it back-fills a
     hole with a LATER item that fits, and every later panel is wide. Marking the
     panel itself is the only thing that works.

     :has() rather than a count in the markup: the panels come from several
     independent {#if} blocks, and counting in Svelte would mean duplicating every
     one of those conditions. */
  .grid > .panel:not(.wide):has(+ .panel.wide) {
    grid-column: 1 / -1;
  }

  @media (max-width: 720px) {
    .grid { grid-template-columns: 1fr; }
    /* Two-up rather than one: the tiles are short, and stacking all three would
       push the panels below the fold on a narrow window. */
    .tiles { grid-template-columns: 1fr 1fr; }
  }

  /* The year in squares: columns are weeks, rows run Monday to Sunday */
  .cal-wrap {
    position: relative;
    overflow-x: auto;
    padding-bottom: 4px;
    /* Width of the weekday-label gutter, shared by the month row and the grid so
       their columns stay in step. Wide enough for the longest label at 10px. */
    --cal-gutter: 22px;
    /* Thickness of the hovered month's ring. Tied to the window rather than fixed:
       the square measures ~19px on a wide window but ~6px on a narrow one, where a
       thick ring covers most of the fill and the shade of the square — the thing
       the heatmap exists to show — stops being readable.

       Whole pixels only. A 1.5px value was tried first and is not achievable: at
       devicePixelRatio 1 the engine floors outline-width to whole pixels, so it
       computed to 1px and the wide window silently kept the thin ring. */
    --cal-ring: 1px;
  }

  /* A viewport media query, not a container query: nothing in the ancestor chain
     declares container-type, so a container query here would silently never
     match. 900px is where the measured cell passes ~10px and can carry 2px. */
  @media (min-width: 900px) {
    .cal-wrap { --cal-ring: 2px; }
  }

  /* One row of month names on the same column track as the grid below, so a label
     sits exactly above the first square of its month. */
  .cal-months {
    display: grid;
    /* The leading gutter is a fixed width shared with .cal-grid below rather than
       `auto`: auto would size to this row's own content, which is empty here and a
       three-letter label there, and the two tracks would drift apart — putting
       every month name a couple of columns off its square. */
    grid-template-columns: var(--cal-gutter) repeat(var(--cal-cols, 53), minmax(0, 1fr));
    gap: 2px;
    margin-bottom: 3px;
    font-size: 10px;
    color: var(--text-secondary);
  }

  .cal-month {
    /* The name is wider than one column, so it is allowed to run over the
       following ones instead of being squeezed or clipped. */
    white-space: nowrap;
    grid-row: 1;
    /* Only as wide as the word: the element stretches across several columns by
       default, and a stretched one would swallow hovers meant for its neighbour. */
    justify-self: start;
    cursor: default;
    transition: color 120ms ease;
  }

  .cal-month.on {
    color: var(--accent);
    font-weight: 650;
  }

  /* The columns are sized by the grid, not by arithmetic in a custom property.
     An earlier attempt put clamp(10px, calc((100% - ...) / 53), 16px) in a
     variable and fed it to grid-template-rows: a percentage there resolves
     against the block axis, whose height is indefinite, so every cell collapsed
     and the year became a thin stripe.

     Columns carry the width, aspect-ratio makes the cell square, and the row
     height follows from that. 53 columns for a full year. */
  .cal-grid {
    display: grid;
    grid-auto-flow: column;
    /* The count comes from the data (--cal-cols) so no cell can land in an
       implicit column, which would be sized by a different rule.

       The track is 1fr with no bound in either direction, so the year always spans
       exactly its panel. A floor (10px) made the grid wider than the panel on a
       narrow window and produced a horizontal scrollbar; a ceiling (16px) left
       slack on a wide one, which is the empty space this block started out with.
       Squareness comes from aspect-ratio on the cell, so a wider column simply
       makes a bigger square. */
    grid-template-columns: var(--cal-gutter) repeat(var(--cal-cols, 53), minmax(0, 1fr));
    grid-template-rows: repeat(7, auto);
    gap: 2px;
  }

  .cal-wd {
    /* Right-aligned against the first square, and vertically centred on its row —
       the row's height comes from the square's aspect-ratio, which is taller than
       a 10px line. */
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding-right: 2px;
    font-size: 10px;
    /* The row height comes from the square's aspect-ratio, and on a narrow window
       a square is smaller than a 10px line. Without this the label's line box
       becomes the taller of the two and sets a floor on the row: measured at a
       700px viewport the squares stayed 6px while the rows stayed 12, so the
       vertical gaps grew to three times the horizontal ones and the year looked
       stretched. Zero height with overflow visible lets the text keep its size
       while the row follows the squares; the labels stay legible because they are
       centred on rows that are only every other one. */
    height: 0;
    line-height: 0;
    overflow: visible;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .cal-cell {
    /* No width/height: the column gives the width and the ratio gives the height.
       min-width:0 lets a button shrink below its intrinsic size — without it the
       default min-width:auto would keep the track from ever reaching 10px. */
    min-width: 0;
    aspect-ratio: 1;
    border-radius: 2px;
    background: var(--bg-secondary);
    border: none;
    padding: 0;
    cursor: pointer;
    transition: outline-color 120ms ease;
    /* Longhands, not the `outline` shorthand: with the shorthand the width came
       out 1px even where --cal-ring resolved to 1.5px, because the shorthand
       resolves its parts once at this declaration and the media query below only
       redefines the variable. Declared transparent rather than left unset so the
       transition has a from-value and the ring fades in instead of jumping. */
    outline-style: solid;
    outline-width: var(--cal-ring);
    outline-color: transparent;
    outline-offset: 0;
  }

  /* Every square of the hovered month is ringed. Deliberately not a background
     wash or a band behind the whole month: the grid runs in week columns, and a
     month starts and ends mid-week, so any rectangle spanning whole columns would
     also cover days of the neighbouring month. A per-square ring is the only
     treatment that matches the month exactly. */
  .cal-cell.on {
    outline-color: var(--accent);
    /* Above its neighbours so the ring is not overdrawn by the next square's
       background, which would nick the shared edges. */
    position: relative;
    z-index: 1;
  }

  .cal-cell.lead { background: transparent; }

  .cal-tip {
    /* fixed, not absolute: the element is rendered at the top level so it can
       overhang the panel instead of forcing .cal-wrap to scroll. */
    position: fixed;
    /* Above the panels but below the modal overlay. */
    z-index: 50;
    max-width: 280px;
    /* Keep CAL_TIP_MAX_H in sync — the flip decision is made against that number
       before the element exists to be measured. */
    max-height: 180px;
    overflow: hidden;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18);
    padding: 6px 10px;
    font-size: 12px;
    pointer-events: none;
  }

  .cal-tip-head { font-weight: 600; }

  .cal-tip-item {
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* The hour-by-weekday heatmap */
  .heat {
    display: grid;
    grid-template-columns: 24px repeat(24, 1fr);
    gap: 2px;
    align-items: center;
  }

  .heat-label {
    font-size: 10px;
    color: var(--text-secondary);
  }

  .heat-cell {
    aspect-ratio: 1 / 1;
    min-width: 0;
    border-radius: 2px;
    background: var(--bg-secondary);
  }

  .heat-hour {
    font-size: 9px;
    color: var(--text-secondary);
    text-align: left;
  }

  .donut-row {
    display: flex;
    align-items: center;
    gap: 18px;
  }

  .donut-total {
    font-size: 22px;
    font-weight: 600;
    fill: var(--text-primary);
  }

  .donut-caption {
    font-size: 10px;
    fill: var(--text-secondary);
  }

  .legend {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 13px;
  }

  .legend li {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .swatch {
    width: 11px;
    height: 11px;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .ratio-row { margin-bottom: 12px; }
  .ratio-row:last-child { margin-bottom: 0; }

  .ratio-head {
    display: flex;
    justify-content: space-between;
    font-size: 13px;
    margin-bottom: 4px;
  }

  .track {
    background: var(--bg-secondary);
    border-radius: 3px;
    height: 7px;
    overflow: hidden;
  }

  .track.tall { height: 14px; flex: 1; }

  .fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
  }

  .fill.done { background: var(--success); }

  .goals {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .goal-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 13px;
    margin-bottom: 4px;
  }

  .goal-name { font-weight: 600; }

  .goal-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 3px;
  }

  .goal-metric {
    font-size: 11px;
    color: var(--text-secondary);
    width: 48px;
    flex-shrink: 0;
  }

  .goal-row .track { flex: 1; }

  .goal-val {
    font-size: 11px;
    min-width: 52px;
    /* min-width (not width) already lets this grow past 52px for the hour form,
       but nowrap keeps "4 ч 32 мин" on one line if the row ever runs short. */
    white-space: nowrap;
    text-align: right;
    flex-shrink: 0;
  }

  /* Label/value rows that fill their panel: a dotted rule carries the eye across
     the gap, so a wide container reads as deliberate rather than as a layout that
     ran out of content. */
  .stat-rows {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .stat-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 12px;
  }

  .stat-row dt {
    color: var(--text-secondary);
    flex-shrink: 0;
  }

  /* The leader: an empty flex-1 element with a bottom border. */
  .stat-row::before {
    content: "";
    order: 1;
    flex: 1;
    border-bottom: 1px dotted var(--border);
    transform: translateY(-3px);
  }

  .stat-row dd {
    order: 2;
    margin: 0;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .ai-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  .apps-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .apps-cols {
    display: flex;
    gap: 24px;
    align-items: flex-start;
  }

  /* .active-toggle is gone: the apps period now uses the shared .seg.
     .btn-group stays, because below it sit the AI summary buttons, and those
     are not a switch — they trigger an action rather than select a state. */
  .btn-group {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }

  .ai-text {
    font-size: 13px;
    line-height: 1.55;
    padding-top: 4px;
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .bar-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

  .activity-more {
    margin-top: 8px;
    color: var(--text-secondary);
  }

  .bar-date {
    width: 78px;
    color: var(--text-secondary);
    flex-shrink: 0;
  }

  .bar-val {
    /* Wide enough for "12 ч 34 мин" and non-wrapping. At the old 52px, sized for
       a bare "384 мин", the hour form broke onto a second line and pushed the
       rows out of alignment. */
    width: 76px;
    white-space: nowrap;
    text-align: right;
    color: var(--text-secondary);
  }

  .dots {
    display: flex;
    gap: 3px;
    flex-wrap: wrap;
  }

  .dot {
    width: 12px;
    height: 12px;
    background: var(--accent);
    border-radius: 3px;
    display: inline-block;
  }

  .backdrop {
    align-items: center;
    padding: 16px;
  }

  .dialog {
    width: 100%;
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

  .actions {
    display: flex;
    justify-content: flex-end;
  }

  .cal-popup {
    max-width: 420px;
  }

  .cal-popup-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 320px;
    overflow-y: auto;
  }

  .cal-popup-item {
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    padding: 6px 8px;
    border-radius: var(--radius);
    font-size: 13px;
    cursor: pointer;
  }

  .cal-popup-item:hover {
    background: var(--bg-secondary);
  }
</style>
