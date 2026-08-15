<script lang="ts">
  import { onMount, tick } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { emit, listen } from "@tauri-apps/api/event";
  import { api } from "../api/tauri";
  import { categoryStore } from "../stores/categories.svelte";
  import { parseClipboardNote } from "../clipboardNote";
  import { splitSubtaskLines, SUBTASK_PREFIX } from "../composer";
  import { parseChecklist, formatChecklist } from "../checklistText";
  import ChecklistEditor from "./ChecklistEditor.svelte";
  import Select from "./Select.svelte";
  import VoiceButton from "./VoiceButton.svelte";
  import { voice } from "../voice.svelte";
  import { applyCachedTheme, applyTheme } from "../theme";
  import type { PinnedItem, Subtask } from "../types";
  import { t } from "../i18n.svelte";
  import "../../app.css";

  // "pinned" is a full third mode, unlike "clipboard": that one folded into "note"
  // because it also created a note. Here the window creates nothing and edits
  // something existing, so it has a form of its own.
  type Mode = "task" | "note" | "pinned";
  let mode = $state<Mode>("task");

  // The pinned item: null means the slot is empty (nothing was pinned, or the
  // object was deleted).
  let pinned = $state<PinnedItem | null>(null);
  let pinnedTitle = $state("");
  let pinnedText = $state("");
  // Refs for voice input (v0.9.65): dictated text is spliced in at the caret, and
  // a textarea gives its caret position only through the element itself.
  let pinnedTextEl: HTMLTextAreaElement | undefined = $state();
  let noteContentEl: HTMLTextAreaElement | undefined = $state();
  let descriptionEl: HTMLTextAreaElement | undefined = $state();
  let saved = $state(false);
  // The pinned task's checklist. Unlike TaskModal, where edits accumulate and leave
  // as a diff on "Save", here every click goes to the DB at once: the slot is opened
  // to tick something off and close, and losing a tick on Escape would sting more
  // than in an editing form.
  //
  // Once the checklist became a text field (as in TaskModal), "at once" needs
  // qualifying: we do not write to the DB on every letter. An edit leaves after a
  // typing pause, and also when the window closes and on "Save". The immediacy
  // guards against losing a tick on Escape, not literally against every keypress.
  let subs = $state<Subtask[]>([]);
  let subsText = $state("");
  let subsBusy = $state(false);
  let subsTimer: ReturnType<typeof setTimeout> | null = null;
  const SUBS_DEBOUNCE_MS = 600;
  // The "text taken from the clipboard" hint, for clipboard mode only. It is removed
  // at the first edit so it does not hang over text that has been changed.
  // "clipboard" itself is not part of Mode: it is not a third tab but a pre-filled
  // note, so in the UI it folds into "note".
  let fromClipboard = $state(false);

  // Task
  let title = $state("");
  let description = $state("");
  let priority = $state("Medium");
  let category = $state("Other"); // the fallback category always exists

  // $derived rather than a plain const: t() reads the current language, and a
  // const would freeze the labels at whatever it was when the module ran. The same
  // shape TaskModal uses for this list — values are the backend's enum spelling.
  const priorities = $derived([
    { value: "Low", label: t("Низкий") },
    { value: "Medium", label: t("Средний") },
    { value: "High", label: t("Высокий") },
    { value: "Critical", label: t("Критический") },
  ]);

  // Note
  let noteTitle = $state("");
  let noteContent = $state("");

  let errorMsg: string | null = $state(null);
  let busy = $state(false);

  // Paint from the localStorage cache first so the window does not flash, then
  // reconcile against the DB in onMount. The cache alone is not enough here, and
  // this window is where that shows: it is written as a side effect of the main
  // window applying a theme, so a quick window opened by a hotkey without the
  // main window ever having run in this webview finds nothing and falls back to
  // applyTheme("system", {}) — which, with no prefers-color-scheme: dark coming
  // from the compositor, renders light while the user has dark set.
  applyCachedTheme();

  // Clipboard mode expands into a note pre-filled from the clipboard. An empty
  // clipboard (or one holding an image or a file) is not an error: an ordinary empty
  // note opens, as with Ctrl+Shift+M.
  async function applyMode(m: string) {
    if (m === "pinned") {
      mode = "pinned";
      fromClipboard = false;
      saved = false;
      // The slot is read on every open rather than cached: the pinned item may have
      // been edited in the main window or deleted.
      pinned = await api.getPinnedItem().catch(() => null);
      pinnedTitle = pinned?.title ?? "";
      pinnedText = pinned?.text ?? "";
      subs = pinned?.subtasks ?? [];
      subsText = formatChecklist(subs.map((s) => ({ title: s.title, done: s.done })));
      return;
    }
    if (m !== "clipboard") {
      mode = m === "note" ? "note" : "task";
      fromClipboard = false;
      return;
    }
    mode = "note";
    const text = await api.readClipboardText().catch(() => "");
    const parsed = parseClipboardNote(text);
    if (!parsed) {
      fromClipboard = false;
      return;
    }
    noteTitle = parsed.title;
    noteContent = parsed.content;
    fromClipboard = true;
  }

  onMount(() => {
    // The initial mode comes from managed state (covering the case where the window
    // was already mounted before the event was emitted).
    api.getQuickMode().then(applyMode).catch(() => {});
    // The authoritative theme. On failure the cached paint above stands rather
    // than being reset to a default — a stale theme beats a wrong one.
    api.getSettings().then((s) => applyTheme(s.theme_mode, s)).catch(() => {});
    categoryStore.load();
    // A live mode change while the window is open.
    const un = listen<string>("quick-mode", (e) => { applyMode(e.payload); });
    return () => { un.then((f) => f()); };
  });

  function reset() {
    title = ""; description = ""; priority = "Medium"; category = "Other";
    noteTitle = ""; noteContent = "";
    errorMsg = null;
    fromClipboard = false;
    // The slot itself and its checklist are not reset: reset clears creation drafts,
    // and a pinned item is not a draft — it lives in the DB and survives the window
    // closing.
    saved = false;
  }

  async function createTask() {
    if (!title.trim() || busy) return;
    busy = true;
    try {
      // The description doubles as the subtask list: Shift+Enter inserts a line
      // prefixed with "☐", and those lines are split off here. The same gesture as
      // the main window's composer, so what is learned in one place holds in the
      // other.
      //
      // splitSubtaskLines rather than parseComposer: the composer is a single
      // field where the first line is the task's title, so parseComposer drops it
      // from the description. Here the title has a field of its own and every line
      // of this one is description — running it through parseComposer silently ate
      // the first line.
      const { text, subtasks } = splitSubtaskLines(description);
      const created = await api.createTask({
        title: title.trim(),
        description: text.trim() || null,
        status: "Todo",
        priority: priority as any,
        category: category as any,
        deadline: null,
        tags: [],
        recurrence: "None",
      });
      for (const sub of subtasks) {
        await api.addSubtask(created.id, sub);
      }
      await emit("task-created");
      await getCurrentWindow().hide();
      reset();
    } catch (e) {
      errorMsg = typeof e === "string" ? e : (e as Error)?.message ?? t("Не удалось создать задачу");
    } finally {
      busy = false;
    }
  }

  async function createNote() {
    if ((!noteTitle.trim() && !noteContent.trim()) || busy) return;
    busy = true;
    try {
      await api.createNote({
        title: noteTitle.trim() || t("Без названия"),
        content: noteContent,
      });
      await emit("note-created");
      await getCurrentWindow().hide();
      reset();
    } catch (e) {
      errorMsg = typeof e === "string" ? e : (e as Error)?.message ?? t("Не удалось создать заметку");
    } finally {
      busy = false;
    }
  }

  // Editing the pinned item. Unlike createTask/createNote the window is NOT hidden
  // after saving: the slot is something one returns to, and adding to it usually
  // takes several passes. Instead of closing, a "Saved" marker appears.
  async function savePinned() {
    if (!pinned || busy) return;
    const title = pinnedTitle.trim();
    // An empty title is rejected: for a task the backend requires one, and for a note
    // it would become an unnamed row in the list.
    if (!title) {
      errorMsg = t("Заголовок не может быть пустым");
      return;
    }
    busy = true;
    errorMsg = null;
    try {
      if (pinned.kind === "task") {
        await flushSubs();
        await api.updateTask(pinned.id, { title, description: pinnedText });
        await emit("task-updated");
      } else {
        await api.updateNote(pinned.id, { title, content: pinnedText });
        await emit("note-updated");
      }
      saved = true;
    } catch (e) {
      errorMsg = typeof e === "string" ? e : (e as Error)?.message ?? t("Не удалось сохранить");
    } finally {
      busy = false;
    }
  }

  // Checklist operations are saved immediately. The scheme is the same for all
  // three: the request first, then the local list is updated — so on an error the
  // screen keeps what is really in the DB rather than something drawn optimistically.
  // Re-reading the whole slot after each click is unnecessary: the title and the text
  // may be under edit right now, and re-reading would clobber that.
  //
  // Editing the checklist: the write is deferred while the user types. The timer
  // restarts on every change, so a finished string reaches the DB rather than one
  // letter per subtask.
  function scheduleSubsFlush() {
    if (subsTimer) clearTimeout(subsTimer);
    subsTimer = setTimeout(() => { subsTimer = null; flushSubs(); }, SUBS_DEBOUNCE_MS);
  }

  // The same positional diff as in TaskModal: line i edits subtask i. The local subs
  // is what really sits in the DB; on an error it is left alone and the next flush
  // retries the same edit.
  async function flushSubs() {
    if (subsTimer) { clearTimeout(subsTimer); subsTimer = null; }
    if (!pinned || pinned.kind !== "task" || subsBusy) return;
    const current = parseChecklist(subsText);
    const orig = subs;
    const same =
      current.length === orig.length &&
      current.every((c, i) => c.title === orig[i].title && c.done === orig[i].done);
    if (same) return;
    subsBusy = true;
    errorMsg = null;
    try {
      const next: Subtask[] = [];
      for (let i = current.length; i < orig.length; i++) {
        await api.deleteSubtask(orig[i].id);
      }
      for (let i = 0; i < current.length; i++) {
        const c = current[i];
        const o = orig[i];
        if (!o) {
          const added = await api.addSubtask(pinned.id, c.title);
          if (c.done) await api.toggleSubtask(added.id);
          next.push({ ...added, done: c.done });
        } else {
          if (o.title !== c.title) await api.renameSubtask(o.id, c.title);
          if (o.done !== c.done) await api.toggleSubtask(o.id);
          next.push({ ...o, title: c.title, done: c.done });
        }
      }
      subs = next;
      await emit("task-updated");
    } catch (e) {
      errorMsg = typeof e === "string" ? e : (e as Error)?.message ?? t("Не удалось сохранить подзадачи");
    } finally {
      subsBusy = false;
    }
  }

  function submit() {
    if (mode === "pinned") savePinned();
    else if (mode === "task") createTask();
    else createNote();
  }

  async function cancel() {
    // A deferred checklist edit is flushed BEFORE hiding the window: Escape is the
    // normal way to close the slot here, and what was typed must not be lost to it.
    await flushSubs();
    await getCurrentWindow().hide();
    reset();
  }

  // Dictation by Ctrl+Shift+D (v0.9.66). The text goes into whichever field holds
  // the caret: this window has three of them depending on the mode, and asking the
  // DOM is the only reliable way to tell which one the user is in.
  //
  // KeyD by e.code rather than e.key: on a Russian layout e.key is "в", and the
  // hotkey must not depend on the layout — the same reason the global hotkeys are
  // stored as "Ctrl+Shift+KeyN".
  async function dictate() {
    if (!(await voice.ensureChecked())) return;
    const el = document.activeElement;
    if (!(el instanceof HTMLTextAreaElement)) return;

    const text = await voice.toggle();
    if (!text) return;

    if (el === pinnedTextEl) {
      pinnedText = insertIntoTextarea(el, pinnedText, text);
      saved = false;
    } else if (el === noteContentEl) {
      noteContent = insertIntoTextarea(el, noteContent, text);
      fromClipboard = false;
    } else if (el === descriptionEl) {
      description = insertIntoTextarea(el, description, text);
    }
  }

  // Shift+Enter starts a subtask line in the description, the same gesture as the
  // main window's composer.
  //
  // From the title it also moves the caret: the title is an <input>, which cannot
  // hold a line break at all, so the combination would otherwise do nothing in the
  // very field that has the focus when the window opens. Jumping to the
  // description is what makes the gesture available from the first keystroke.
  function insertSubtaskLine() {
    const el = descriptionEl;
    if (!el) return;
    const fromTitle = document.activeElement !== el;
    // At the end of existing text a bare prefix is enough; mid-text it needs the
    // break first, or the subtask would be glued onto the line already there.
    const at = fromTitle ? description.length : (el.selectionStart ?? description.length);
    const to = fromTitle ? description.length : (el.selectionEnd ?? at);
    const before = description.slice(0, at);
    const needsBreak = before !== "" && !before.endsWith("\n");
    const insert = (needsBreak ? "\n" : "") + SUBTASK_PREFIX;
    description = before + insert + description.slice(to);
    const caret = at + insert.length;
    // After the DOM has the new value: setting it first would leave the caret at
    // the end of the field on the next keystroke.
    tick().then(() => {
      el.focus();
      el.setSelectionRange(caret, caret);
    });
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.ctrlKey && e.shiftKey && e.code === "KeyD") {
      e.preventDefault();
      void dictate();
      return;
    }
    // Ctrl+Tab switches the creation tabs. In pinned-editing mode there are no tabs —
    // moving away into a creation form would mean abandoning an unsaved edit, so that
    // mode ignores the switch.
    if (e.ctrlKey && e.key === "Tab") {
      e.preventDefault();
      if (mode !== "pinned") mode = mode === "task" ? "note" : "task";
      return;
    }
    if (e.key === "Escape") { cancel(); return; }
    // Shift+Enter starts a subtask line — creation of a task only. A note has no
    // subtasks, and in the pinned slot the checklist is a field of its own with its
    // own editor, so the combination belongs to that editor there.
    if (e.key === "Enter" && e.shiftKey && mode === "task") {
      e.preventDefault();
      insertSubtaskLine();
      return;
    }
    // Enter creates: for a task from any field, for a note only with Ctrl (a plain
    // Enter in a textarea inserts a line break). Editing a pinned item behaves like a
    // note: it is multi-line text as well.
    if (e.key === "Enter" && !e.shiftKey) {
      if (mode === "task") { e.preventDefault(); submit(); }
      else if (e.ctrlKey) { e.preventDefault(); submit(); }
    }
  }

  // Inserts dictated text into a textarea at the caret, replacing the selection —
  // the same thing typing would do (v0.9.65). Unlike the note editor there is no
  // CodeMirror here, so the value is spliced by hand and the caret is put after the
  // insertion; without that it would jump to the start on the next keystroke.
  //
  // Spaces are added on whichever side abuts a word character: whisper returns a
  // bare phrase, so dictating mid-sentence would otherwise glue it to the
  // neighbouring word on that side.
  function insertIntoTextarea(el: HTMLTextAreaElement, current: string, text: string): string {
    const from = el.selectionStart ?? current.length;
    const to = el.selectionEnd ?? from;
    const before = current.slice(0, from);
    const after = current.slice(to);
    const padLeft = before !== "" && !/\s$/.test(before);
    const padRight = after !== "" && !/^\s/.test(after);
    const insert = (padLeft ? " " : "") + text + (padRight ? " " : "");
    const next = before + insert + after;
    // The caret is restored after the DOM has the new value, otherwise the browser
    // resets it to the end of the field.
    queueMicrotask(() => {
      el.focus();
      el.setSelectionRange(from + insert.length, from + insert.length);
    });
    return next;
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="container">
  {#if mode !== "pinned"}
    <!-- Tabs as plain text rather than a .seg control. The window has exactly one
         job, and a bordered segmented control at the top competed with the field
         below it for first attention — everything weighed the same and the eye had
         nowhere to land. Underlined text still reads as a choice while staying
         quieter than the input it introduces.

         This is deliberately NOT the shared .seg: that control exists to make a
         toggle read as one object among many on a busy screen, and here there is
         nothing to be one of. The exception is written down so a later pass does
         not "unify" it back — the same note .seg--underline carries in app.css. -->
    <div class="tabs">
      <button class="tab" class:active={mode === "task"} onclick={() => mode = "task"}>{t("Задача")}</button>
      <button class="tab" class:active={mode === "note"} onclick={() => mode = "note"}>{t("Заметка")}</button>
      <span class="tabs-spacer"></span>
      <kbd>Ctrl Tab</kbd>
    </div>
  {/if}

  {#if errorMsg}
    <p class="error">{errorMsg}</p>
  {/if}

  {#if mode === "pinned"}
    {#if pinned}
      <div class="pin-head">
        <span class="pin-badge">⚡ {pinned.kind === "task" ? t("Задача") : t("Заметка")}</span>
        {#if saved}<span class="pin-saved">{t("Сохранено")}</span>{/if}
      </div>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="pin-title" bind:value={pinnedTitle} placeholder={t("Заголовок...")}
        oninput={() => saved = false} />
      <div class="field-with-voice">
        <textarea class="pin-text" bind:this={pinnedTextEl} bind:value={pinnedText} placeholder={t("Текст... (Ctrl+Enter — сохранить)")}
          rows={pinned.kind === "task" ? 3 : 6} autofocus oninput={() => saved = false}></textarea>
        <VoiceButton onText={(text) => {
          if (!pinnedTextEl) return;
          pinnedText = insertIntoTextarea(pinnedTextEl, pinnedText, text);
          saved = false;
        }} />
      </div>

      <!-- The checklist belongs to a task only: notes have no subtasks. Edits here
           reach the DB by themselves (after a typing pause, and also on Escape and
           "Save"), so the "Save" button below is about the title and the text. -->
      {#if pinned.kind === "task"}
        <div class="subs">
          <div class="subs-head">
            <span class="subs-label">{t("Подзадачи")}</span>
          </div>
          <ChecklistEditor
            bind:value={subsText}
            placeholder={t("Подзадача на строку (Enter — ещё строка)")}
            onchange={scheduleSubsFlush}
          />
        </div>
      {/if}

      <div class="buttons">
        <button class="btn-ghost" onclick={cancel}>{t("Закрыть")}</button>
        <button class="btn-primary" onclick={savePinned} disabled={busy || !pinnedTitle.trim()}>
          {t("Сохранить")}
        </button>
      </div>
    {:else}
      <!-- An empty slot is not an error: the user has pinned nothing yet, or what was
           pinned has been deleted. We explain how to pin instead of showing an
           empty window. -->
      <div class="pin-empty">
        <p class="pin-empty-title">{t("⚡ Слот пуст")}</p>
        <p class="pin-empty-hint">
          {t("Закрепите задачу или заметку кнопкой-молнией в списке — этот хоткей будет открывать её сразу на правку.")}
        </p>
      </div>
      <div class="buttons">
        <button class="btn-ghost" onclick={cancel}>{t("Закрыть")}</button>
      </div>
    {/if}
  {:else if mode === "task"}
    <!-- The title carries the window. It is borderless and larger than anything
         else here so the window reads as "type here" at a glance instead of as a
         form to be filled in top to bottom. -->
    <!-- svelte-ignore a11y_autofocus -->
    <input class="lead-input" bind:value={title} placeholder={t("Название задачи...")} autofocus />

    <!-- The description is always present rather than behind a "+ описание"
         toggle. The toggle cost a click and a re-layout to reveal a field that was
         going to be typed into anyway, and it sat in the same row as the two
         selects, which is what made that row read as three unrelated controls
         crammed together. Kept visually quiet so it does not compete with the
         title above it. -->
    <div class="field-with-voice desc-field">
      <!-- The placeholder is where this gesture is discoverable: nothing else on
           the screen hints that Shift+Enter does anything. Same wording as the
           main window's composer, which carries the same hint. -->
      <textarea class="desc-input" bind:this={descriptionEl} bind:value={description}
        placeholder={t("Описание... (Shift+Enter — подзадача)")}></textarea>
      <VoiceButton onText={(text) => {
        if (!descriptionEl) return;
        description = insertIntoTextarea(descriptionEl, description, text);
      }} />
    </div>

    <!-- The footer holds what qualifies the task rather than what the task is:
         parameters on the left, the commit on the right. Separating them by role
         is what lets the eye skip the row entirely when the defaults are fine. -->
    <div class="footer">
      <div class="params">
        <Select
          value={priority}
          options={priorities}
          onChange={(v) => priority = v}
          ariaLabel={t("Приоритет")}
        />
        <Select
          value={category}
          options={categoryStore.categories.map((c) => ({ value: c.id, label: categoryStore.name(c.id) }))}
          onChange={(v) => category = v}
          ariaLabel={t("Категория")}
        />
      </div>
      <button class="btn-ghost" onclick={cancel}>{t("Отмена")}</button>
      <button class="btn-primary" onclick={createTask} disabled={!title.trim()}>{t("Создать")}</button>
    </div>
  {:else}
    {#if fromClipboard}
      <p class="clip-hint">{t("Текст из буфера обмена — можно поправить перед сохранением")}</p>
    {/if}
    <!-- svelte-ignore a11y_autofocus -->
    <input class="lead-input" bind:value={noteTitle} placeholder={t("Заголовок заметки...")} autofocus
      oninput={() => fromClipboard = false} />
    <div class="field-with-voice desc-field">
      <textarea class="desc-input" bind:this={noteContentEl} bind:value={noteContent}
        placeholder={t("Текст заметки... (Ctrl+Enter — сохранить)")}
        oninput={() => fromClipboard = false}></textarea>
      <VoiceButton onText={(text) => {
        if (!noteContentEl) return;
        noteContent = insertIntoTextarea(noteContentEl, noteContent, text);
        fromClipboard = false;
      }} />
    </div>

    <!-- No .params here: a note has nothing to qualify. The footer keeps its shape
         so the commit button does not jump between the two tabs. -->
    <div class="footer">
      <span class="tabs-spacer"></span>
      <button class="btn-ghost" onclick={cancel}>{t("Отмена")}</button>
      <button class="btn-primary" onclick={createNote} disabled={!noteTitle.trim() && !noteContent.trim()}>{t("Создать")}</button>
    </div>
  {/if}
</div>

<style>
  .container {
    padding: 14px 16px 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-primary);
    height: 100vh;
    box-sizing: border-box;
  }
  /* No blanket gap. Every group used to be 8px from every other, so the tabs, the
     title, the selects and the buttons all read as one undifferentiated stack —
     which is what made a window with four controls feel cramped. Spacing is set
     per boundary instead, by how related the two sides are. */
  .tabs {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-bottom: 10px;
  }
  .tabs-spacer {
    flex: 1;
  }
  /* Text, not a segmented control: see the markup. The underline is the same
     gradient the active tab carries in the main window, so "this one is current"
     looks the same in both places. */
  .tab {
    padding: 2px 0;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-size: 13px;
    position: relative;
  }
  .tab:hover {
    background: transparent;
    color: var(--text-primary);
  }
  .tab.active {
    color: var(--text-primary);
    font-weight: 500;
  }
  .tab.active::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: -3px;
    height: 2px;
    border-radius: 1px;
    background: linear-gradient(90deg, var(--accent), var(--accent-secondary));
  }
  .error {
    font-size: 12px;
    color: var(--danger);
    margin: 0;
  }
  .clip-hint {
    font-size: 11px;
    color: var(--text-secondary);
    margin: 0;
  }
  /* The one field the window is about. Borderless and larger than the body text:
     a boxed input the same size as everything else made the window read as a form,
     and the first thing to do here is type. The focus ring is dropped with it —
     the caret already sits in this field on open, and a ring around the only lead
     element is noise. */
  .lead-input {
    border: none;
    background: transparent;
    padding: 0;
    font-size: 17px;
    font-weight: 500;
    line-height: 1.3;
  }
  .lead-input:focus {
    outline: none;
    border-color: transparent;
  }
  .lead-input::placeholder {
    color: var(--text-secondary);
    font-weight: 400;
  }
  /* The secondary field fills whatever is left between the title and the footer,
     so the window has no dead space at the bottom regardless of its height. */
  .desc-field {
    flex: 1;
    min-height: 0;
    margin-top: 10px;
  }
  .desc-input {
    flex: 1;
    min-height: 0;
    border: none;
    background: transparent;
    padding: 0;
    resize: none;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .desc-input:focus {
    outline: none;
    border-color: transparent;
    color: var(--text-primary);
  }
  textarea {
    resize: none;
    font-size: 13px;
  }
  /* The microphone sits in the field's bottom-right corner rather than beside it:
     the quick-capture window is narrow, and a button in the row would squeeze the
     text. The wrapper is a flex item like the textarea it replaces, so the
     surrounding layout is unchanged. */
  .field-with-voice {
    position: relative;
    display: flex;
    flex-direction: column;
  }
  .field-with-voice :global(.voice-btn) {
    position: absolute;
    right: 6px;
    bottom: 6px;
    /* Semi-transparent so it does not hide the last line of text under it. */
    opacity: 0.75;
  }
  .field-with-voice :global(.voice-btn:hover),
  .field-with-voice :global(.voice-btn.recording) {
    opacity: 1;
  }
  .buttons {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 2px;
  }
  /* The footer is a band rather than a row of buttons: it spans the full width by
     cancelling the container's side padding, and its own background separates
     "what the thing is" above from "how it is filed" below. The negative margin is
     what lets the band touch the window edges while the fields stay inset. */
  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px -16px 0;
    padding: 10px 16px;
    border-top: 1px solid var(--border);
    background: var(--bg-card);
  }
  .params {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    /* The two dropdowns share the room left of the buttons instead of sizing to
       their labels: a category named "Другое" against one named "Саморазвитие"
       made the pair visibly lopsided. */
    min-width: 0;
  }
  .params :global(button) {
    font-size: 12px;
    padding: 3px 8px;
    max-width: 100%;
  }
  /* Editing the pinned item. A creation form is a "blank sheet"; here, by
     contrast, it matters to see at a glance that something existing is being
     edited, hence the header with the type and the accent border around the
     text. */
  .pin-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .pin-badge {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    letter-spacing: 0.02em;
  }
  .pin-saved {
    font-size: 11px;
    color: var(--text-secondary);
    margin-left: auto;
  }
  .pin-title {
    font-weight: 600;
  }
  .pin-text {
    flex: 1;
    resize: none;
    font-size: 13px;
    border-left: 2px solid var(--accent);
  }
  /* The checklist in the slot, separated from the text by a heading with a
     counter — without it the two groups of fields merge into one wall. */
  .subs {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-height: 0;
    overflow-y: auto;
  }
  .subs-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 2px;
  }
  .subs-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
  }
  /* The checklist rows and the counter moved into ChecklistEditor: deleting a
     row means deleting text, so a separate cross is unnecessary. */
  .pin-empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 6px;
    text-align: center;
  }
  .pin-empty-title {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }
  .pin-empty-hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
    line-height: 1.4;
  }
</style>
