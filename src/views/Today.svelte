<script lang="ts">
  import { onMount } from "svelte";
  import { taskStore } from "../lib/stores/tasks.svelte";
  import { categoryStore } from "../lib/stores/categories.svelte";
  import PomodoroWidget from "../lib/components/PomodoroWidget.svelte";
  import Icon from "../lib/components/Icon.svelte";
  import TaskOpener from "../lib/components/TaskOpener.svelte";
  import type { Task } from "../lib/types";

  // `t` is taken here by a local variable, so the translation helper is imported as `tr`.
  import { t as tr, i18n } from "../lib/i18n.svelte";
  import { hhmm, pad2, localeTag } from "../lib/datetime";
  // A task opens right here, without leaving for the Tasks screen
  let openTaskId = $state<string | null>(null);

  const HOUR_H = 48; // px per hour: tighter than the Calendar's week grid but enough for a block's 2 lines
  const DAY_START_H = 6;
  const DAY_END_H = 24;

  let now = $state(new Date());
  onMount(() => {
    taskStore.load();
    categoryStore.load();
    const tick = setInterval(() => now = new Date(), 30_000);
    return () => clearInterval(tick);
  });

  function isToday(iso: string): boolean {
    const d = new Date(iso);
    return d.toDateString() === now.toDateString();
  }

  // Today's blocks: the same selection as "day-plan" in Tasks.svelte (the chip
  // list), rendered here on a single day's vertical timeline.
  const todayBlocks = $derived.by(() =>
    taskStore.activeTasks
      .filter(t => t.scheduled_at && isToday(t.scheduled_at))
      .sort((a, b) => a.scheduled_at!.localeCompare(b.scheduled_at!))
  );

  function blockTop(t: Task): number {
    const d = new Date(t.scheduled_at!);
    const mins = (d.getHours() - DAY_START_H) * 60 + d.getMinutes();
    return (mins / 60) * HOUR_H;
  }
  function blockHeight(t: Task): number {
    return Math.max(((t.scheduled_mins ?? 60) / 60) * HOUR_H, 22);
  }
  // Below 34px the time and the title do not fit one above the other, so we show them on one line.
  function blockCompact(t: Task): boolean {
    return blockHeight(t) < 34;
  }
  function blockRange(t: Task): string {
    const start = new Date(t.scheduled_at!);
    const end = new Date(start.getTime() + (t.scheduled_mins ?? 60) * 60_000);
    return `${hhmm(start)}–${hhmm(end)}`;
  }

  const nowLineTop = $derived.by(() => {
    const mins = (now.getHours() - DAY_START_H) * 60 + now.getMinutes();
    return (mins / 60) * HOUR_H;
  });
  const showNowLine = $derived(now.getHours() >= DAY_START_H && now.getHours() < DAY_END_H);

  const hours = Array.from({ length: DAY_END_H - DAY_START_H }, (_, i) => DAY_START_H + i);

  // A compact deadline in the same format as Tasks.svelte::deadlineInfo.
  function deadlineInfo(iso: string): { label: string; overdue: boolean } {
    const d = new Date(iso);
    const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const dayDiff = Math.round((startOfDay(d) - startOfDay(now)) / 864e5);
    if (d.getTime() < now.getTime()) {
      return { label: dayDiff === 0 ? tr("просрочено") : tr("просрочено {n} дн", { n: -dayDiff }), overdue: true };
    }
    if (dayDiff === 0) return { label: tr("сегодня {time}", { time: d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) }), overdue: false };
    return { label: "", overdue: false };
  }

  // Deadlines today plus overdue ones: not time blocks but a separate task list.
  const dueTasks = $derived.by(() =>
    taskStore.activeTasks
      .filter(t => t.status !== "Done" && t.deadline)
      .filter(t => isToday(t.deadline!) || new Date(t.deadline!).getTime() < now.getTime())
      .sort((a, b) => a.deadline!.localeCompare(b.deadline!))
  );

  // The day's progress: the share of completed tasks among those that belong to
  // today (due today OR blocked today), plus anything completed today with neither.
  const todayRelevant = $derived.by(() => {
    const ids = new Set<string>();
    const list: Task[] = [];
    for (const t of taskStore.tasks) {
      const blockToday = t.scheduled_at && isToday(t.scheduled_at);
      const deadlineToday = t.deadline && isToday(t.deadline);
      const completedToday = t.completed_at && isToday(t.completed_at);
      if (blockToday || deadlineToday || completedToday) {
        if (!ids.has(t.id)) { ids.add(t.id); list.push(t); }
      }
    }
    return list;
  });
  const dayProgress = $derived.by(() => {
    const total = todayRelevant.length;
    const done = todayRelevant.filter(t => t.status === "Done").length;
    return { total, done, pct: total > 0 ? Math.round((done / total) * 100) : 0 };
  });

  async function completeTask(id: string) {
    await taskStore.complete(id);
  }
</script>

<div class="today-view">
  <header class="today-header">
    <h2>
      {tr("Сегодня")}
      <span class="muted today-date">{now.toLocaleDateString(localeTag(i18n.lang), { weekday: "long", day: "numeric", month: "long" })}</span>
    </h2>
    {#if dayProgress.total > 0}
      <div class="day-progress" title={tr("{done} из {total} выполнено", { done: dayProgress.done, total: dayProgress.total })}>
        <div class="day-progress-track"><div class="day-progress-fill" style="width:{dayProgress.pct}%"></div></div>
        <span class="muted">{dayProgress.done}/{dayProgress.total}</span>
      </div>
    {/if}
  </header>

  <div class="today-body">
    <section class="today-col today-timeline-col">
      <h3 class="col-title"><Icon name="calendar" size={13} /> {tr("Блоки на сегодня")}</h3>
      {#if todayBlocks.length === 0}
        <p class="muted empty-hint">{tr("На сегодня блоков не запланировано.")}</p>
      {:else}
        <div class="timeline" style="height:{(DAY_END_H - DAY_START_H) * HOUR_H}px">
          {#each hours as h}
            <div class="hour-line" style="top:{(h - DAY_START_H) * HOUR_H}px">
              <span class="hour-label">{pad2(h)}:00</span>
            </div>
          {/each}
          {#if showNowLine}
            <div class="now-line" style="top:{nowLineTop}px"></div>
          {/if}
          {#each todayBlocks as t (t.id)}
            <button
              class="tl-block"
              class:compact={blockCompact(t)}
              style="top:{blockTop(t)}px; height:{blockHeight(t)}px;"
              onclick={() => openTaskId = t.id}
              title="{blockRange(t)} — {t.title}"
            >
              <span class="tl-block-time">{blockRange(t)}</span>
              <span class="tl-block-title">{t.title}</span>
            </button>
          {/each}
        </div>
      {/if}
    </section>

    <section class="today-col today-side-col">
      <div class="side-card">
        <PomodoroWidget />
      </div>

      <div class="side-card">
        <h3 class="col-title"><Icon name="flag" size={13} /> {tr("Дедлайны сегодня и просрочка")}</h3>
        {#if dueTasks.length === 0}
          <p class="muted empty-hint">{tr("Ничего срочного.")}</p>
        {:else}
          <ul class="due-list">
            {#each dueTasks as t (t.id)}
              {@const dl = deadlineInfo(t.deadline!)}
              <li class="due-row">
                <button
                  class="task-check"
                  onclick={() => completeTask(t.id)}
                  title={tr("Выполнить")}
                  aria-label={tr("Выполнить задачу")}
                ></button>
                <button class="due-main" onclick={() => openTaskId = t.id}>
                  <span class="prio-dot" style="--prio: var(--prio-{t.priority.toLowerCase()});"></span>
                  <span class="due-title">{t.title}</span>
                </button>
                <span class="chip chip-cat" style="--cat: {categoryStore.color(t.category)}">{categoryStore.name(t.category)}</span>
                <span class="chip" class:chip-danger={dl.overdue}>{dl.label}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </section>
  </div>
</div>

{#if openTaskId}
  <TaskOpener taskId={openTaskId} onClose={() => openTaskId = null} />
{/if}

<style>
  /* The same ceiling as Tasks: this screen is a list of cards too, and letting
     it run to 1900px would put it out of step with the view it mirrors. The
     padding stays outside the limit so the content is centred, not the padding. */
  .today-view {
    padding: 20px 24px;
    height: 100%;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .today-view > * {
    width: 100%;
    max-width: 1200px;
    margin-inline: auto;
  }

  .today-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 10px;
  }

  .today-header h2 {
    margin: 0;
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .today-date {
    font-size: 13px;
    font-weight: 400;
    text-transform: capitalize;
  }

  .day-progress {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

  .day-progress-track {
    width: 140px;
    height: 6px;
    border-radius: 3px;
    background: var(--bg-secondary);
    overflow: hidden;
  }

  .day-progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.3s ease;
  }

  .today-body {
    display: flex;
    gap: 20px;
    flex: 1;
    min-height: 0;
    align-items: flex-start;
  }

  .today-col {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .today-timeline-col {
    flex: 1;
    min-width: 0;
  }

  .today-side-col {
    width: 280px;
    flex-shrink: 0;
  }

  .side-card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
  }

  .col-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    margin: 0 0 4px 0;
    color: var(--text-secondary);
  }

  .empty-hint {
    font-size: 12px;
    margin: 0;
  }

  .timeline {
    position: relative;
    border-left: 1px solid var(--border);
    margin-left: 44px;
  }

  .hour-line {
    position: absolute;
    left: 0;
    right: 0;
    border-top: 1px solid var(--border);
  }

  .hour-label {
    position: absolute;
    left: -44px;
    top: -7px;
    width: 38px;
    text-align: right;
    font-size: 10px;
    color: var(--text-secondary);
  }

  .now-line {
    position: absolute;
    left: 0;
    right: 0;
    height: 2px;
    background: var(--danger);
    z-index: 2;
  }

  .tl-block {
    position: absolute;
    left: 4px;
    right: 4px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: center;
    gap: 1px;
    padding: 3px 8px;
    border: none;
    border-left: 3px solid var(--accent);
    border-radius: 4px;
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-card));
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
    overflow: hidden;
  }

  .tl-block.compact {
    flex-direction: row;
    align-items: center;
    gap: 6px;
  }

  .tl-block:hover {
    background: color-mix(in srgb, var(--accent) 20%, var(--bg-card));
  }

  .tl-block-time {
    font-size: 10px;
    color: var(--text-secondary);
    flex-shrink: 0;
  }

  .tl-block-title {
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .due-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .due-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 2px;
    border-radius: 4px;
  }

  .due-row:hover {
    background: var(--bg-hover);
  }

  .due-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: transparent;
    padding: 0;
    text-align: left;
    cursor: pointer;
    color: var(--text-primary);
    font: inherit;
  }

  .due-title {
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .prio-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--prio, var(--prio-low));
  }

  .task-check {
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    padding: 0;
    border-radius: 50%;
    border: 1.5px solid var(--text-secondary);
    background: transparent;
  }

  .task-check:hover {
    border-color: var(--success);
    background: color-mix(in srgb, var(--success) 15%, transparent);
  }
</style>
