<script lang="ts">
  import type { Task, CreateTaskPayload, UpdateTaskPayload, Priority, Category, Recurrence, RecurrenceUnit, TaskStatus, ChecklistTemplate } from "../types";
  import ChecklistEditor from "./ChecklistEditor.svelte";
  import Select from "./Select.svelte";
  import { parseChecklist, formatChecklist } from "../checklistText";
  import { api } from "../api/tauri";
  import { projectStore } from "../stores/projects.svelte";
  import { categoryStore } from "../stores/categories.svelte";
  import { statusStore } from "../stores/statuses.svelte";
  import { taskStore } from "../stores/tasks.svelte";
  import { t } from "../i18n.svelte";
  import { toLocalInput } from "../datetime";

  type Props = {
    task?: Task | null;
    initialDeadline?: string | null; // deadline prefill on creation (datetime-local format)
    initialStatus?: TaskStatus; // status prefill on creation (kanban: the column sets it)
    // Returns the created task (in create mode) so the modal can append the subtasks
    // from the inline checklist; in edit mode the return value is unused.
    onSave: (data: CreateTaskPayload | UpdateTaskPayload) => Promise<Task | null | void>;
    onClose: () => void;
  };

  let { task = null, initialDeadline = null, initialStatus = "Todo", onSave, onClose }: Props = $props();

  const isEdit = !!task;

  // Blocker candidates: open tasks other than this one and those already added.
  // Cycles are rejected by the backend itself, so they are not filtered here —
  // otherwise the whole dependency graph would have to be pulled into the frontend
  // just for a hint. The blockers are kept in local state rather than read from the
  // prop: taskStore.load() creates NEW task objects after an edit and the prop keeps
  // pointing at the old one, so the list in an open modal would not update until it
  // was reopened.
  let blockedBy = $state(task?.blocked_by ?? []);

  const candidateBlockers = $derived(
    taskStore.tasks.filter(c =>
      c.id !== task?.id &&
      !c.hidden &&
      !blockedBy.some(b => b.id === c.id),
    ),
  );

  // Takes the id rather than the element: the control is a Select component now,
  // and there is no field to reset — it is bound to a constant "", so it always
  // shows the placeholder and never displays an already-added blocker.
  async function addBlocker(blockerId: string) {
    if (!blockerId || !task) return;
    await taskStore.addDependency(task.id, blockerId);
    blockedBy = taskStore.tasks.find(x => x.id === task!.id)?.blocked_by ?? [];
  }

  async function removeBlocker(blockerId: string) {
    if (!task) return;
    await taskStore.removeDependency(task.id, blockerId);
    blockedBy = taskStore.tasks.find(x => x.id === task!.id)?.blocked_by ?? [];
  }

  // The modal is also opened by sections that never loaded categories or statuses (the Calendar)
  if (categoryStore.categories.length === 0) categoryStore.load();
  if (statusStore.statuses.length === 0) statusStore.load();

  let title = $state(task?.title ?? "");
  let description = $state(task?.description ?? "");
  let status = $state<TaskStatus>(task?.status ?? initialStatus);
  let priority = $state<Priority>(task?.priority ?? "Medium");
  // "Other" is the fallback category and always exists (unlike Work, which can be deleted)
  let category = $state<Category>(task?.category ?? "Other");
  let tagsInput = $state((task?.tags ?? []).join(", "));
  let totalTaskMins = $state(0);

  if (task) {
    api.getTaskSeconds(task.id).then(s => totalTaskMins = Math.round(s / 60)).catch(() => {});
  }
  // "" means no project; in a patch an empty string detaches it
  let projectId = $state(task?.project_id ?? "");

  let deadline = $state(task?.deadline ? toLocalInput(task.deadline) : (initialDeadline ?? ""));

  type RecurrenceKey = "None" | "Hourly" | "Daily" | "Weekly" | "Custom" | "Weekdays";

  function initRecurrenceKey(): RecurrenceKey {
    const r = task?.recurrence;
    if (!r || r === "None") return "None";
    if (r === "Hourly") return "Hourly";
    if (r === "Daily") return "Daily";
    if (r === "Weekly") return "Weekly";
    if (typeof r === "object" && r !== null && "Weekdays" in r) return "Weekdays";
    return "Custom";
  }

  let recurrenceKey = $state<RecurrenceKey>(initRecurrenceKey());
  function initCustomN(): number {
    const r = task?.recurrence;
    if (typeof r === "object" && r !== null && "Custom" in r) return r.Custom[0];
    return 1;
  }
  function initCustomUnit(): RecurrenceUnit {
    const r = task?.recurrence;
    if (typeof r === "object" && r !== null && "Custom" in r) return r.Custom[1];
    return "Hours";
  }

  let customN = $state(initCustomN());
  let customUnit = $state<RecurrenceUnit>(initCustomUnit());

  // Weekdays for Recurrence::Weekdays — the same pattern as days_mask on routines
  // (RoutinesModal.svelte): bit 0 = Monday ... bit 6 = Sunday.
  const WEEKDAY_LABELS = $derived([t("Пн"), t("Вт"), t("Ср"), t("Чт"), t("Пт"), t("Сб"), t("Вс")]);
  function initWeekdays(): boolean[] {
    const r = task?.recurrence;
    if (typeof r === "object" && r !== null && "Weekdays" in r) {
      return WEEKDAY_LABELS.map((_, i) => (r.Weekdays & (1 << i)) !== 0);
    }
    return WEEKDAY_LABELS.map(() => false);
  }
  let weekdays = $state<boolean[]>(initWeekdays());
  function weekdaysMask(): number {
    return weekdays.reduce((acc, on, i) => acc | (on ? 1 << i : 0), 0);
  }

  let saving = $state(false);
  let error = $state("");

  // --- The subtask checklist as a single text field. A line is a subtask and the
  // `[x]`/`[ ]` prefix is the tick. Every line used to be its own <input>, which made
  // it impossible to move through the list with arrow keys or select several lines at
  // once. Changes are still applied on save (a diff against task.subtasks) rather than
  // immediately.
  //
  // The correspondence "line <-> existing subtask" is kept positionally: an id does
  // not survive in text, and the alternative (hidden markers inside the text) would
  // break the very thing a text field is for. The practical consequence is that a
  // reordered line counts as a rename rather than a move; for a checklist of a few
  // items that is cheaper than matching by content.
  // svelte-ignore state_referenced_locally -- the modal is recreated for each task ({#if editingTask}), so a snapshot of the initial value is exactly what is wanted here
  let subsText = $state(
    formatChecklist((task?.subtasks ?? []).map(s => ({ title: s.title, done: s.done }))),
  );
  const hasSubs = $derived(parseChecklist(subsText).length > 0);

  // --- Checklist templates ---
  let checklistTemplates: ChecklistTemplate[] = $state([]);
  let templatePickerOpen = $state(false);
  let savingTemplateOpen = $state(false);
  let newTemplateName = $state("");

  async function loadChecklistTemplates() {
    checklistTemplates = await api.getChecklistTemplates().catch(() => []);
  }

  function toggleTemplatePicker() {
    templatePickerOpen = !templatePickerOpen;
    savingTemplateOpen = false;
    if (templatePickerOpen) loadChecklistTemplates();
  }

  function applyTemplate(template: ChecklistTemplate) {
    // A template is appended to whatever is already typed rather than replacing it.
    const added = formatChecklist(template.items.map(title => ({ title, done: false })));
    subsText = subsText.trim() ? `${subsText.replace(/\n+$/, "")}\n${added}` : added;
    templatePickerOpen = false;
  }

  async function removeTemplate(id: string) {
    await api.deleteChecklistTemplate(id);
    await loadChecklistTemplates();
  }

  function toggleSaveTemplate() {
    savingTemplateOpen = !savingTemplateOpen;
    templatePickerOpen = false;
    newTemplateName = "";
  }

  async function saveCurrentAsTemplate() {
    const name = newTemplateName.trim();
    const items = parseChecklist(subsText).map(s => s.title);
    if (!name || items.length === 0) return;
    await api.createChecklistTemplate(name, items);
    savingTemplateOpen = false;
    newTemplateName = "";
  }

  // The checklist diffed against the task's original subtasks. Lines of text are
  // matched to existing subtasks positionally (no id is stored in the text, see the
  // comment on subsText): line i edits subtask i, extra lines are added and extra
  // subtasks removed. That way editing the wording stays a rename and keeps the tick
  // rather than recreating the subtask from scratch.
  async function applySubtaskChanges(taskId: string) {
    const orig = task?.subtasks ?? [];
    const current = parseChecklist(subsText);
    for (let i = current.length; i < orig.length; i++) {
      await api.deleteSubtask(orig[i].id);
    }
    for (let i = 0; i < current.length; i++) {
      const s = current[i];
      const o = orig[i];
      if (!o) {
        const added = await api.addSubtask(taskId, s.title);
        if (s.done) await api.toggleSubtask(added.id);
      } else {
        if (o.title !== s.title) await api.renameSubtask(o.id, s.title);
        if (o.done !== s.done) await api.toggleSubtask(o.id);
      }
    }
  }

  function buildRecurrence(): Recurrence {
    switch (recurrenceKey) {
      case "Hourly":   return "Hourly";
      case "Daily":    return "Daily";
      case "Weekly":   return "Weekly";
      case "Custom":   return { Custom: [customN, customUnit] };
      case "Weekdays": {
        const mask = weekdaysMask();
        return mask === 0 ? "None" : { Weekdays: mask };
      }
      default:         return "None";
    }
  }

  function parseTags(s: string): string[] {
    return s.split(",").map(t => t.trim()).filter(Boolean);
  }

  async function handleSave() {
    if (!title.trim()) { error = t("Название не может быть пустым"); return; }
    if (recurrenceKey === "Weekdays" && weekdaysMask() === 0) {
      error = t("Выберите хотя бы один день недели");
      return;
    }
    saving = true;
    error = "";
    try {
      const recurrence = buildRecurrence();
      // The deadline is no longer cleared for a recurring task — it is the time of the
      // first occurrence, meaning the same as it does without recurrence (see
      // next_occurrence on the backend, which shifts this very field on completion).
      const deadlineIso = deadline ? new Date(deadline).toISOString() : null;

      if (isEdit) {
        // Subtasks go before onSave: onSave updates the task and re-reads the store,
        // picking up the checklist changes along the way.
        await applySubtaskChanges(task!.id);
        const patch: UpdateTaskPayload = {
          title: title.trim(),
          description: description.trim() || undefined,
          status,
          priority,
          category,
          tags: parseTags(tagsInput),
          recurrence,
          project_id: projectId,
          ...(deadlineIso ? { deadline: deadlineIso } : {}),
        };
        await onSave(patch);
      } else {
        const payload: CreateTaskPayload = {
          title: title.trim(),
          description: description.trim() || null,
          status,
          priority,
          category,
          tags: parseTags(tagsInput),
          recurrence,
          deadline: deadlineIso,
          project_id: projectId || null,
        };
        const created = await onSave(payload);
        const newSubs = parseChecklist(subsText);
        if (created && "id" in created && newSubs.length > 0) {
          for (const s of newSubs) {
            const added = await api.addSubtask(created.id, s.title);
            if (s.done) await api.toggleSubtask(added.id);
          }
          await taskStore.load(); // create already re-read the store BEFORE the subtasks
        }
      }
      onClose();
    } catch (e) {
      error = typeof e === "string" ? e : t("Ошибка при сохранении");
    } finally {
      saving = false;
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }

  function handleKeydown(e: KeyboardEvent) {
    // A belt-and-braces guard for controls that handle a key themselves. The
    // dropdown's own stopPropagation already keeps Escape from reaching this
    // window listener, so nothing currently depends on this line — it is here so
    // that a control which only calls preventDefault does not close the modal and
    // lose unsaved edits.
    if (e.defaultPrevented) return;
    if (e.key === "Escape") onClose();
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") handleSave();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div role="dialog" aria-modal="true" class="overlay backdrop" onclick={handleBackdropClick}>
  <div class="modal dialog">
    <h2 class="dialog-title">{isEdit ? t("Редактировать задачу") : t("Новая задача")}</h2>

    {#if error}
      <div class="alert" style="margin:0;">{error}</div>
    {/if}

    <label class="field">
      <span class="label">{t("Название *")}</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={title} placeholder={t("Название задачи")} autofocus />
    </label>

    <label class="field">
      <span class="label">{t("Описание")}</span>
      <textarea bind:value={description} placeholder={t("Описание (необязательно)")} rows="3" style="resize:vertical;"></textarea>
    </label>

    <div class="field">
      <span class="label">{t("Подзадачи")}</span>
      <ChecklistEditor bind:value={subsText} placeholder={t("Подзадача на строку (Enter — ещё строка)")} />
      <div class="template-row">
        <button type="button" class="btn-sm" onclick={toggleTemplatePicker}>{t("Из шаблона…")}</button>
        <button type="button" class="btn-sm" onclick={toggleSaveTemplate}
          disabled={!hasSubs}
          title={hasSubs ? "" : t("Сначала добавьте подзадачи")}>
          {t("Сохранить как шаблон")}
        </button>
      </div>

      {#if templatePickerOpen}
        <div class="template-panel">
          {#if checklistTemplates.length === 0}
            <span class="muted" style="font-size:12px;">{t("Нет сохранённых шаблонов")}</span>
          {:else}
            {#each checklistTemplates as tpl (tpl.id)}
              <div class="template-line">
                <span style="flex:1;">{tpl.name} <span class="muted">({tpl.items.length})</span></span>
                <button type="button" class="btn-sm" onclick={() => applyTemplate(tpl)}>{t("Применить")}</button>
                <button type="button" class="btn-icon btn-danger" title={t("Удалить шаблон")} onclick={() => removeTemplate(tpl.id)}>✕</button>
              </div>
            {/each}
          {/if}
        </div>
      {/if}

      {#if savingTemplateOpen}
        <div class="template-panel template-line">
          <input
            type="text"
            placeholder={t("Название шаблона")}
            bind:value={newTemplateName}
            onkeydown={(e) => { if (e.key === 'Enter') saveCurrentAsTemplate(); }}
            class="sub-input"
          />
          <button type="button" class="btn-sm btn-primary" onclick={saveCurrentAsTemplate} disabled={!newTemplateName.trim()}>
            {t("Сохранить")}
          </button>
        </div>
      {/if}
    </div>

    <div class="pair">
      <!-- A <div>, not a <label>: the control is a button now, and a label wrapping
           a button would toggle the dropdown on every click of the caption. -->
      <div class="field">
        <span class="label">{t("Приоритет")}</span>
        <Select
          value={priority}
          onChange={(v) => (priority = v as Priority)}
          ariaLabel={t("Приоритет")}
          options={[
            { value: "Low", label: t("Низкий") },
            { value: "Medium", label: t("Средний") },
            { value: "High", label: t("Высокий") },
            { value: "Critical", label: t("Критический") },
          ]}
        />
      </div>
      <div class="field">
        <span class="label">{t("Категория")}</span>
        <Select
          value={category}
          onChange={(v) => (category = v as Category)}
          ariaLabel={t("Категория")}
          options={categoryStore.categories.map(c => ({ value: c.id, label: categoryStore.name(c.id) }))}
        />
      </div>
    </div>

    {#if isEdit && totalTaskMins > 0}
      <div class="field">
        <span class="label">{t("Время всего")}</span>
        <span class="muted" style="font-size:13px;">{t("{n} мин", { n: totalTaskMins })}</span>
      </div>
    {/if}

    {#if isEdit}
      <div class="field">
        <span class="label">{t("Статус")}</span>
        <Select
          value={status}
          onChange={(v) => (status = v as TaskStatus)}
          ariaLabel={t("Статус")}
          options={statusStore.statuses.map(s => ({ value: s.id, label: statusStore.name(s.id) }))}
        />
      </div>
    {/if}

    <div class="field recurrence-block">
      <span class="label">{t("Дедлайн и повтор")}</span>
      <div class="pair">
        <label class="field">
          <span class="sublabel">{recurrenceKey === "None" ? t("Дедлайн") : t("Первое срабатывание")}</span>
          <input type="datetime-local" bind:value={deadline} />
        </label>
        <div class="field">
          <span class="sublabel">{t("Повтор")}</span>
          <Select
            value={recurrenceKey}
            onChange={(v) => (recurrenceKey = v as typeof recurrenceKey)}
            ariaLabel={t("Повтор")}
            options={[
              { value: "None", label: t("Без повтора") },
              { value: "Hourly", label: t("Каждый час") },
              { value: "Daily", label: t("Каждый день") },
              { value: "Weekly", label: t("Каждую неделю") },
              { value: "Custom", label: t("Свой интервал") },
              { value: "Weekdays", label: t("По дням недели") },
            ]}
          />
        </div>
      </div>

      {#if recurrenceKey === "Custom"}
        <div class="custom-row">
          <span>{t("Каждые")}</span>
          <input type="number" bind:value={customN} min="1" style="width:64px;" />
          <div class="custom-unit">
            <Select
              value={customUnit}
              onChange={(v) => (customUnit = v as RecurrenceUnit)}
              ariaLabel={t("Единица интервала")}
              options={[
                { value: "Minutes", label: t("минут") },
                { value: "Hours", label: t("часов") },
                { value: "Days", label: t("дней") },
                { value: "Weeks", label: t("недель") },
              ]}
            />
          </div>
        </div>
      {/if}

      {#if recurrenceKey === "Weekdays"}
        <div class="day-picker">
          {#each WEEKDAY_LABELS as d, i}
            <label class="day-chip">
              <input type="checkbox" bind:checked={weekdays[i]} />
              <span>{d}</span>
            </label>
          {/each}
        </div>
      {/if}

      {#if recurrenceKey !== "None"}
        <span class="hint">{t("При выполнении задача не закрывается — дедлайн сам сдвинется на следующий срок, задача останется активной.")}</span>
      {/if}
    </div>

    <label class="field">
      <span class="label">{t("Теги (через запятую)")}</span>
      <input bind:value={tagsInput} placeholder={t("работа, важное, срочное")} />
    </label>

    {#if projectStore.active.length > 0 || projectId}
      <div class="field">
        <span class="label">{t("Проект")}</span>
        <Select
          value={projectId}
          onChange={(v) => (projectId = v)}
          ariaLabel={t("Проект")}
          options={[
            { value: "", label: t("Без проекта") },
            ...projectStore.active.map(p => ({ value: p.id, label: p.name })),
            // a task may hang on an archived project — we do not lose the link
            ...projectStore.projects
              .filter(p => p.archived && p.id === projectId)
              .map(p => ({ value: p.id, label: `${p.name} (${t("архив")})` })),
          ]}
        />
      </div>
    {/if}

    <!-- Dependencies exist only for a saved task: the link is written into a
         separate table keyed by an id a new task does not have yet. Edits here
         apply immediately rather than on "Save", just like subtasks. -->
    {#if isEdit && task}
      <div class="field">
        <span class="label">{t("Блокируется задачами")}</span>
        {#if blockedBy.length > 0}
          <ul class="blockers">
            {#each blockedBy as b (b.id)}
              <li>
                <span class="blocker-title">{b.title}</span>
                <button
                  class="btn-ghost blocker-del"
                  onclick={() => removeBlocker(b.id)}
                  title={t("Убрать зависимость")}
                  aria-label={t("Убрать зависимость")}
                >×</button>
              </li>
            {/each}
          </ul>
        {/if}
        <!-- An action rather than a stored value: it always shows the placeholder
             and adds whatever is picked, so there is nothing to bind to. -->
        <Select
          value=""
          placeholder={t("Добавить блокер...")}
          ariaLabel={t("Добавить блокер...")}
          onChange={addBlocker}
          options={candidateBlockers.map(c => ({ value: c.id, label: c.title }))}
        />
      </div>
    {/if}

    <div class="actions">
      <span class="muted" style="font-size:11px;margin-right:auto;"><kbd>Ctrl Enter</kbd> {t("сохранить ·")} <kbd>Esc</kbd> {t("закрыть")}</span>
      <button class="btn-ghost" onclick={onClose}>{t("Отмена")}</button>
      <button class="btn-primary" onclick={handleSave} disabled={saving || !title.trim()}>
        {saving ? t("Сохранение...") : isEdit ? t("Сохранить") : t("Создать")}
      </button>
    </div>
  </div>
</div>

<style>
  /* Dependencies */
  .blockers {
    list-style: none;
    margin: 0 0 6px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .blockers li {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--bg-hover);
    border-radius: var(--radius);
    padding: 4px 6px 4px 9px;
    font-size: 12px;
  }

  .blocker-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .blocker-del {
    padding: 0 6px;
    line-height: 1;
    font-size: 15px;
  }

  .backdrop {
    align-items: center;
    padding: 16px;
  }

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

  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }

  /* Deadline and recurrence are grouped visually: a shared border explains that
     this is one connected setting rather than two independent fields. */
  .recurrence-block {
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    gap: 8px;
  }

  .sublabel {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .hint {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .custom-row {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 13px;
  }

  /* The unit sits in a flex row next to "Каждые" and a number field. The native
     select it replaced was sized by its widest option; the Select button fills its
     container instead, so the width is set here rather than letting it stretch
     across the rest of the row. */
  .custom-unit { width: 120px; }

  .day-picker {
    display: flex;
    gap: 4px;
  }

  .day-chip {
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: 12px;
  }

  .day-chip input {
    margin: 0;
  }

  /* The subtask checklist moved into ChecklistEditor — the row styles live there
     too, and only the row of template buttons remains here. */
  .template-row {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }

  .template-panel {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    margin-top: 4px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
  }

  .template-line {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .sub-input {
    flex: 1;
    font-size: 12px;
    padding: 2px 8px;
  }

  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: flex-end;
    margin-top: 4px;
  }
</style>
