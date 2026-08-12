<script lang="ts">
  import { onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { noteStore } from "../lib/stores/notes.svelte";
  import { taskStore } from "../lib/stores/tasks.svelte";
  import { projectStore } from "../lib/stores/projects.svelte";
  import { pinnedStore } from "../lib/stores/pinned.svelte";
  import { api } from "../lib/api/tauri";
  import { extractWikiLinks, renderMarkdown } from "../lib/markdown";
  import { t, tErr, i18n } from "../lib/i18n.svelte";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import Icon from "../lib/components/Icon.svelte";
  import VoiceButton from "../lib/components/VoiceButton.svelte";
  import Select from "../lib/components/Select.svelte";
  import ContextMenu, { type MenuItem } from "../lib/components/ContextMenu.svelte";
  import type { Note, NoteRevision } from "../lib/types";
  import { localDateKey, toLocalInput, localeTag } from "../lib/datetime";
  import { isTypingTarget, actionForKey, nextIndex, reconcileIndex } from "../lib/listnav";
  import { loadUiState, saveUiState } from "../lib/uistate";
  type EditorExports = { focus: () => void; formatBold: () => void; formatItalic: () => void; formatCode: () => void; formatHeading: () => void; formatChecklist: () => void; formatWikiLink: () => void; formatQuote: () => void; formatOrderedList: () => void; formatLink: () => void; insertTable: () => void; replaceRange: (from: number, to: number, text: string) => void; insertAtCursor: (text: string) => void };
  let editorRef: EditorExports | undefined = $state();

  let selectedId: string | null = $state(null);
  let dailyKey = $state(0); // tracks dailyRequested
  let editTitle = $state("");
  let editContent = $state("");
  let editTags: string[] = $state([]);
  let editLinkedTaskId: string | null = $state(null);
  let editProjectId: string | null = $state(null);
  // Reminder: datetime-local holds local time, "" means no reminder.
  let editReminderAt = $state("");
  let tagInput = $state("");
  let saveTimeout: ReturnType<typeof setTimeout> | null = null;
  let saving = $state(false);
  let renameToast: string | null = $state(null);
  let renameToastTimeout: ReturnType<typeof setTimeout> | null = null;
  let zenMode = $state(false);
  // The list and the Trash are mutually exclusive, like the segmented toggle in Tasks.
  let listSubView: "notes" | "trash" = $state("notes");

  const selected = $derived(noteStore.notes.find(n => n.id === selectedId) ?? null);
  const otherTitles = $derived(noteStore.notes.filter(n => n.id !== selectedId).map(n => n.title));

  // The notes list filter
  let noteFilter = $state("");
  let filterTag = $state("");
  let filterProjectId = $state("");
  const allTags = $derived([...new Set(noteStore.notes.flatMap(n => n.tags))].sort());
  // Pinned notes always come first, and stably: otherwise the order within a group
  // would jump around for equal pinned values. Array.prototype.sort is guaranteed
  // stable since ES2019, so the backend's order (updated_at DESC) is preserved
  // inside each group.
  const filteredNotes = $derived(noteStore.notes.filter(n => {
    if (noteFilter && !n.title.toLowerCase().includes(noteFilter.toLowerCase())) return false;
    if (filterTag && !n.tags.includes(filterTag)) return false;
    if (filterProjectId && n.project_id !== filterProjectId) return false;
    return true;
  }).sort((a, b) => Number(b.pinned) - Number(a.pinned)));

  async function togglePin(note: Note, e: MouseEvent) {
    e.stopPropagation(); // clicking the pin button must not open the note
    await noteStore.update(note.id, { pinned: !note.pinned });
  }

  // --- Multi-select for notes, following the one for tasks: the same pattern,
  // where Ctrl toggles a row and Shift selects a range from the last selected row
  // in the order of the visible (filtered) list.
  let selectedNoteIds = $state<Set<string>>(new Set());
  let lastSelectedNoteId: string | null = $state(null);
  let bulkNotesBusy = $state(false);
  let bulkNotesProjectId = $state("");

  $effect(() => {
    const visible = new Set(filteredNotes.map(n => n.id));
    if ([...selectedNoteIds].some(id => !visible.has(id))) {
      selectedNoteIds = new Set([...selectedNoteIds].filter(id => visible.has(id)));
    }
  });

  function toggleNoteSelect(note: Note, e: MouseEvent) {
    const ids = filteredNotes.map(n => n.id);
    if (e.shiftKey && lastSelectedNoteId) {
      const from = ids.indexOf(lastSelectedNoteId);
      const to = ids.indexOf(note.id);
      if (from >= 0 && to >= 0) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        const next = new Set(selectedNoteIds);
        for (let i = lo; i <= hi; i++) next.add(ids[i]);
        selectedNoteIds = next;
        return;
      }
    }
    const next = new Set(selectedNoteIds);
    if (next.has(note.id)) next.delete(note.id); else next.add(note.id);
    selectedNoteIds = next;
    lastSelectedNoteId = note.id;
  }

  function onNoteRowClick(e: MouseEvent, note: Note) {
    if (e.ctrlKey || e.metaKey || e.shiftKey) {
      e.preventDefault();
      toggleNoteSelect(note, e);
      return;
    }
    selectNote(note);
  }

  function clearNoteSelection() {
    selectedNoteIds = new Set();
    lastSelectedNoteId = null;
  }

  async function bulkDeleteNotes() {
    bulkNotesBusy = true;
    try {
      await Promise.all([...selectedNoteIds].map(id => api.deleteNote(id)));
      if (selectedId && selectedNoteIds.has(selectedId)) selectedId = null;
      await noteStore.load();
      clearNoteSelection();
    } finally {
      bulkNotesBusy = false;
    }
  }

  async function bulkMoveNotesToProject() {
    if (!bulkNotesProjectId) return;
    bulkNotesBusy = true;
    try {
      const project_id = bulkNotesProjectId === "none" ? null : bulkNotesProjectId;
      await Promise.all([...selectedNoteIds].map(id => api.updateNote(id, { project_id })));
      await noteStore.load();
      clearNoteSelection();
      bulkNotesProjectId = "";
    } finally {
      bulkNotesBusy = false;
    }
  }

  // Notes referring to the current one via [[title]] (case-insensitively).
  const backlinks = $derived.by<Note[]>(() => {
    if (!selectedId) return [];
    const title = editTitle.trim().toLowerCase();
    if (!title) return [];
    return noteStore.notes.filter(n =>
      n.id !== selectedId && extractWikiLinks(n.content).some(l => l.toLowerCase() === title)
    );
  });

  function findByTitle(title: string): Note | null {
    const key = title.trim().toLowerCase();
    return noteStore.notes.find(n => n.title.trim().toLowerCase() === key) ?? null;
  }

  // Writes the title and content and, if the title actually changed, updates the
  // [[links]] in the other notes. oldTitle comes from a stale snapshot
  // (selected.title before this save), not from editTitle, which already holds the
  // new value.
  async function persistNote(id: string, oldTitle: string, newTitle: string, content: string) {
    await noteStore.update(id, { title: newTitle, content });
    const trimmed = newTitle.trim();
    if (trimmed && trimmed.toLowerCase() !== oldTitle.trim().toLowerCase()) {
      const count = await api.renameNoteLinks(oldTitle, trimmed);
      if (count > 0) {
        await noteStore.load();
        if (renameToastTimeout) clearTimeout(renameToastTimeout);
        renameToast = t("Обновлено ссылок: {n}", { n: count });
        renameToastTimeout = setTimeout(() => { renameToast = null; }, 4000);
      }
    }
  }

  // A deferred save must not be lost when switching notes: we clear the timer and
  // write immediately, while selectedId and editContent still point at the old one.
  async function flushPendingSave() {
    if (!saveTimeout) return;
    clearTimeout(saveTimeout);
    saveTimeout = null;
    if (selectedId) {
      const before = selected?.title ?? editTitle;
      await persistNote(selectedId, before, editTitle, editContent);
    }
    saving = false;
  }

  // Remembered on selection rather than through an $effect on selectedId: deleting
  // a note sets selectedId to null, and an effect would then erase the memory
  // instead of leaving the previous note to be reopened next time.
  async function selectNote(note: Note) {
    saveUiState({ noteId: note.id });
    await flushPendingSave();
    suppressNextContentSave = true;
    selectedId = note.id;
    editTitle = note.title;
    editContent = note.content;
    editTags = [...note.tags];
    editLinkedTaskId = note.linked_task_id;
    editProjectId = note.project_id;
    editReminderAt = note.reminder_at ? toLocalInput(note.reminder_at) : "";
    linkSuggestions = null;
    selectionMenu = null;
    selectionResult = null;
    summaryResult = null;
    extractedTasks = null;
  }

  // CodeMirror changes editContent directly through bind:value (with no oninput
  // hook), so autosave hangs off an $effect. suppressNextContentSave suppresses the
  // run triggered by selectNote itself (a programmatic swap, not typing).
  let suppressNextContentSave = false;
  $effect(() => {
    editContent;
    if (suppressNextContentSave) { suppressNextContentSave = false; return; }
    scheduleSave();
  });

  async function openWikiLink(title: string) {
    const existing = findByTitle(title);
    if (existing) {
      selectNote(existing);
      return;
    }
    const created = await noteStore.create({ title, content: "" });
    if (created) selectNote(created);
  }

  async function openDailyNote() {
    const today = new Date();
    const title = localDateKey(today);
    const existing = findByTitle(title);
    if (existing) { selectNote(existing); return; }
    // Yesterday's date
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);
    const created = await noteStore.create({ title, content: `[[${localDateKey(yesterday)}]]\n\n` });
    if (created) selectNote(created);
  }

  // Opening a note on a signal from global search (Ctrl+K).
  $effect(() => {
    const id = noteStore.focusNoteId;
    if (!id) return;
    const note = noteStore.notes.find(n => n.id === id);
    if (note) selectNote(note);
    noteStore.clearFocus();
  });

  // The "open today's note" signal (Ctrl+D from another section).
  $effect(() => {
    dailyKey;
    if (noteStore.dailyRequested === 0) return;
    dailyKey = noteStore.dailyRequested;
    openDailyNote();
  });

  async function newNote() {
    const note = await noteStore.create({ title: t("Без названия"), content: "" });
    if (note) selectNote(note);
  }

  function scheduleSave() {
    if (!selectedId) return;
    if (saveTimeout) clearTimeout(saveTimeout);
    saving = true;
    const id = selectedId;
    const before = selected?.title ?? editTitle;
    saveTimeout = setTimeout(async () => {
      await persistNote(id, before, editTitle, editContent);
      saving = false;
    }, 800);
  }

  // Tags and the task link are saved immediately (no debounce).
  async function saveMeta() {
    if (!selectedId) return;
    await noteStore.update(selectedId, {
      tags: editTags,
      linked_task_id: editLinkedTaskId,
      project_id: editProjectId,
      reminder_at: editReminderAt ? new Date(editReminderAt).toISOString() : null,
    });
  }

  function addTag() {
    const t = tagInput.trim();
    if (t && !editTags.includes(t)) {
      editTags = [...editTags, t];
      saveMeta();
    }
    tagInput = "";
  }

  function removeTag(tag: string) {
    editTags = editTags.filter(t => t !== tag);
    saveMeta();
  }

  function onTagKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") { e.preventDefault(); addTag(); }
  }

  async function deleteSelected() {
    if (!selectedId) return;
    // A deferred save of a note being deleted is pointless — we just clear the timer.
    if (saveTimeout) { clearTimeout(saveTimeout); saveTimeout = null; }
    saving = false;
    // The revisions panel may have been open on this very note. Its revisions now
    // survive the Trash (they are only dropped by purge), but the note itself
    // leaves the list, so the panel has nothing left to point at — close it.
    revisionsOpen = false;
    viewingRevisionId = null;
    await noteStore.remove(selectedId);
    selectedId = null;
    editTitle = "";
    editContent = "";
    editTags = [];
    editLinkedTaskId = null;
  }

  // Zen mode: a fullscreen editor with no list or meta panel, toggled by
  // Ctrl+Shift+Z (not one of the rebindable KEYBIND_ACTIONS — it is local to the
  // Notes section rather than global navigation) and left with Escape. Selecting
  // another note or leaving the section closes the mode silently via the $effect
  // below, otherwise one could get "stuck" in zen with someone else's note.
  function toggleZen() {
    zenMode = !zenMode;
  }
  // One window listener for the whole screen, not two: zen mode and list navigation
  // both watch Escape, and two independent handlers would have raced over it.
  function onZenKeydown(e: KeyboardEvent) {
    if (e.ctrlKey && e.shiftKey && e.code === "KeyZ" && selected) {
      e.preventDefault();
      toggleZen();
      return;
    }
    if (e.key === "Escape" && zenMode) {
      zenMode = false;
      return;
    }
    onListKeydown(e);
  }

  // --- Keyboard navigation over the list (v0.9.77) ---
  //
  // The cursor is a separate thing from `selectedId`: moving it must not open a
  // note, or every j/k would fire a load and a debounced save cycle. Enter opens.
  let focusedIndex = $state(-1);

  // Following the row through re-sorts: pinning or renaming a note reorders the
  // list, and a bare index would silently point at a different note afterwards.
  let focusedId: string | null = $state(null);
  $effect(() => {
    const ids = filteredNotes.map(n => n.id);
    const next = reconcileIndex(focusedId, ids, focusedIndex);
    if (next !== focusedIndex) focusedIndex = next;
    focusedId = next >= 0 ? ids[next] : null;
  });

  function moveFocus(delta: number) {
    const next = nextIndex(focusedIndex, delta, filteredNotes.length);
    focusedIndex = next;
    focusedId = next >= 0 ? filteredNotes[next].id : null;
    if (next >= 0) {
      // The row must be brought into view; "nearest" scrolls the minimum amount, so
      // stepping down a long list does not jerk the row to the middle each time.
      const el = document.querySelector<HTMLElement>(`[data-note-index="${next}"]`);
      el?.scrollIntoView({ block: "nearest" });
    }
  }

  // Delete goes through selectNote first, on purpose: deleteSelected() also cancels
  // the pending autosave and closes the revisions panel, and those must apply to the
  // note actually being deleted. Deleting straight from the row would leave a
  // debounced save in flight for a note that is on its way to the Trash.
  async function deleteFocused(note: Note) {
    await selectNote(note);
    await deleteSelected();
  }

  function onListKeydown(e: KeyboardEvent) {
    // The Trash has no cursor: its rows have no "open", and Delete there would mean
    // purging forever — too destructive to sit under a bare keystroke.
    if (listSubView !== "notes" || zenMode) return;
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

    const note = focusedIndex >= 0 ? filteredNotes[focusedIndex] : null;
    if (!note) return;
    // Space opens too: a note has nothing to "complete", and leaving it unhandled
    // would scroll the page instead.
    if (action === "open" || action === "complete") {
      e.preventDefault();
      selectNote(note);
    } else if (action === "delete") {
      e.preventDefault();
      deleteFocused(note);
    }
  }
  $effect(() => {
    if (!selectedId) zenMode = false;
  });

  // The date locale follows the chosen language rather than being hardcoded to
  // "ru-RU": otherwise dates would stay Russian in an English interface. Elsewhere
  // the project formats through `[]`, i.e. the system locale; here the chosen
  // language is what matters, because it may have been switched by hand.
  function formatDate(iso: string) {
    return new Date(iso).toLocaleDateString(localeTag(i18n.lang),
      { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
  }

  const linkedTask = $derived(
    editLinkedTaskId ? taskStore.tasks.find(t => t.id === editLinkedTaskId) ?? null : null
  );

  // --- AI auto-linking: "Suggest links" ---
  let aiEnabled = $state(false);
  let linkSuggesting = $state(false);
  let linkSuggestions: { noteId: string; titles: string[]; error: string | null } | null = $state(null);

  async function suggestLinks() {
    if (!selectedId) return;
    linkSuggesting = true;
    linkSuggestions = null;
    try {
      await api.aiSuggestLinks(selectedId);
    } catch (e) {
      linkSuggesting = false;
      linkSuggestions = { noteId: selectedId, titles: [], error: String(e) };
    }
  }

  function acceptLinkSuggestion(title: string) {
    const sep = editContent && !editContent.endsWith("\n") ? "\n" : "";
    editContent = `${editContent}${sep}[[${title}]]`; // saving happens via the $effect on editContent
    linkSuggestions = linkSuggestions
      ? { ...linkSuggestions, titles: linkSuggestions.titles.filter(t => t !== title) }
      : null;
  }

  // --- AI on an editor selection: select text -> an action menu appears beside it
  // -> the model proposes a replacement -> confirm or cancel. The same
  // suggest-then-confirm pattern as auto-linking above, only with a choice among
  // four actions and a preview of the result instead of a list of chips.
  type SelectionMenu = { text: string; from: number; to: number; left: number; top: number };
  type SelectionAction = "rewrite" | "shorten" | "expand" | "grammar";
  const SELECTION_ACTION_LABELS: Record<SelectionAction, string> = $derived({
    rewrite: t("Переписать"), shorten: t("Сократить"), expand: t("Развернуть"), grammar: t("Грамматика"),
  });
  let selectionMenu: SelectionMenu | null = $state(null);
  let selectionBusy = $state(false);
  let selectionResult: { requestId: string; text: string; error: string | null } | null = $state(null);
  let selectionRequestId: string | null = null;

  function onEditorSelectionChange(sel: SelectionMenu | null) {
    // While a request is in flight or a result is shown, the selection menu must
    // not jump to a new range under the cursor.
    if (selectionBusy || selectionResult) return;
    selectionMenu = sel;
  }

  async function runSelectionAction(action: SelectionAction) {
    if (!selectionMenu || !aiEnabled) return;
    const requestId = crypto.randomUUID();
    selectionRequestId = requestId;
    selectionBusy = true;
    selectionResult = null;
    try {
      await api.aiEditSelection(requestId, selectionMenu.text, action);
    } catch (e) {
      selectionBusy = false;
      selectionResult = { requestId, text: "", error: String(e) };
    }
  }

  function acceptSelectionResult() {
    if (!selectionMenu || !selectionResult || selectionResult.error) return;
    editorRef?.replaceRange(selectionMenu.from, selectionMenu.to, selectionResult.text);
    selectionMenu = null;
    selectionResult = null;
  }

  function dismissSelectionResult() {
    selectionResult = null;
    selectionMenu = null;
  }

  // --- AI note summary: a small window with 3-5 bullet points, where clicking the
  // text copies it to the clipboard and closes the window. A read-only result with
  // no confirm-into-document step (unlike auto-linking or AI on a selection): the
  // summary is not inserted into the note, it is simply there to be copied
  // anywhere — a chat, another note, a task.
  let summarizing = $state(false);
  let summaryResult: { requestId: string; text: string; error: string | null } | null = $state(null);
  let summaryRequestId: string | null = null;
  let summaryCopied = $state(false);

  async function summarizeNote() {
    if (!selected) return;
    const requestId = crypto.randomUUID();
    summaryRequestId = requestId;
    summarizing = true;
    summaryResult = null;
    summaryCopied = false;
    try {
      await api.aiSummarizeNote(requestId, editContent);
    } catch (e) {
      summarizing = false;
      summaryResult = { requestId, text: "", error: String(e) };
    }
  }

  async function copySummaryAndClose() {
    if (!summaryResult || summaryResult.error) return;
    try {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      await writeText(summaryResult.text);
      summaryCopied = true;
    } catch {
      // the clipboard is unavailable — we still close the window below, just without a copy
    }
    setTimeout(() => { summaryResult = null; summaryCopied = false; }, summaryCopied ? 400 : 0);
  }

  function closeSummary() {
    summaryResult = null;
    summaryCopied = false;
  }

  // --- AI extraction of tasks from a note: suggest-then-confirm, like auto-linking
  // and the summary. The model only proposes a list; every task is created by an
  // explicit click (or all at once via "Accept all"), and nothing is created
  // automatically.
  let extractingTasks = $state(false);
  // A list of plain strings was replaced by rows with state: each item has its own
  // id, a checkbox and editable text.
  //
  // The key is an id rather than the text: the model often produces two similar
  // items, and the previous filter `items.filter(t => t !== title)` deleted both.
  //
  // The model's wording is almost always a draft ("call the workshop" instead of
  // "call the workshop and book Thursday"), so the text is edited right here,
  // before creation, rather than afterwards through the task modal.
  type ExtractedTask = { id: string; title: string; checked: boolean };
  type ExtractedState = { requestId: string; items: ExtractedTask[]; error: string | null };
  let extractedTasks = $state<ExtractedState | null>(null);
  let extractRequestId: string | null = null;
  let creatingExtractedTask = $state(false);

  const extractedChecked = $derived(
    extractedTasks?.items.filter((i: ExtractedTask) => i.checked && i.title.trim()) ?? [],
  );

  async function extractTasks() {
    if (!selected) return;
    const requestId = crypto.randomUUID();
    extractRequestId = requestId;
    extractingTasks = true;
    extractedTasks = null;
    try {
      await api.aiExtractTasks(requestId, editContent);
    } catch (e) {
      extractingTasks = false;
      extractedTasks = { requestId, items: [], error: String(e) };
    }
  }

  // Creates only what is ticked. Items leave the list as they are created, so an
  // operation interrupted halfway does not create duplicates when retried.
  async function createSelectedExtracted() {
    if (!extractedTasks || creatingExtractedTask) return;
    creatingExtractedTask = true;
    try {
      for (const item of [...extractedTasks.items]) {
        const title = item.title.trim();
        if (!item.checked || !title) continue;
        await api.createTask({
          title, description: null, status: "Todo", priority: "Medium",
          category: "Other", deadline: null, tags: [], recurrence: "None",
          project_id: editProjectId,
        });
        extractedTasks = extractedTasks
          ? { ...extractedTasks, items: extractedTasks.items.filter((i) => i.id !== item.id) }
          : null;
      }
      await taskStore.load();
      // An empty list closes itself: there is no point keeping a "0 of 0 created" panel.
      if (extractedTasks && extractedTasks.items.length === 0) extractedTasks = null;
    } finally {
      creatingExtractedTask = false;
    }
  }

  function toggleExtracted(id: string) {
    if (!extractedTasks) return;
    extractedTasks = {
      ...extractedTasks,
      items: extractedTasks.items.map((i) => (i.id === id ? { ...i, checked: !i.checked } : i)),
    };
  }

  function setExtractedTitle(id: string, title: string) {
    if (!extractedTasks) return;
    extractedTasks = {
      ...extractedTasks,
      items: extractedTasks.items.map((i) => (i.id === id ? { ...i, title } : i)),
    };
  }

  function toggleAllExtracted() {
    if (!extractedTasks) return;
    const allOn = extractedTasks.items.every((i) => i.checked);
    extractedTasks = {
      ...extractedTasks,
      items: extractedTasks.items.map((i) => ({ ...i, checked: !allOn })),
    };
  }

  function closeExtractedTasks() {
    extractedTasks = null;
  }

  // --- Note revisions ---
  let revisionsOpen = $state(false);
  let revisions: NoteRevision[] = $state([]);
  let viewingRevisionId: string | null = $state(null);
  let viewingRevisionContent = $state("");
  let revisionsBusy = $state(false);

  // --- Exporting a note to HTML ---
  // Images are stored as files on disk (images_dir/<uuid>.<ext>) and resolved in the
  // editor through asset:// (convertFileSrc). An export must be a self-contained
  // file, so instead of asset:// we embed the images' contents as data: URIs
  // directly in the HTML.
  let exporting = $state(false);

  // The editor header's overflow menu. Anchored under the "…" button rather than
  // at a cursor position: it is opened by a click on a control, not by a right
  // click on a row, so it has to line up with that control on every open.
  let noteMenu = $state<{ x: number; y: number } | null>(null);

  function openNoteMenu(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    // Left edge of the button, just below it. ContextMenu flips this back into
    // the window itself if the menu would not fit.
    noteMenu = { x: r.left, y: r.bottom + 4 };
  }

  // Built where it is used rather than inlined in the markup: the disabled flags
  // come from four separate in-flight states, and repeating those conditions in
  // the template is what made the icon row hard to read in the first place.
  const noteMenuItems: MenuItem[] = $derived([
    ...(aiEnabled ? [{
      label: t("ИИ предложит заметки для связи"),
      disabled: linkSuggesting,
      onSelect: suggestLinks,
    }] : []),
    { label: t("Версии заметки"), onSelect: openRevisions },
    ...(aiEnabled ? [
      { label: t("ИИ: резюме заметки"), disabled: summarizing, onSelect: summarizeNote },
      { label: t("ИИ: извлечь задачи из заметки"), disabled: extractingTasks, onSelect: extractTasks },
    ] : []),
    { label: t("Экспорт в HTML"), disabled: exporting, separated: true, onSelect: exportNoteAsHtml },
    { label: t("Удалить заметку"), danger: true, separated: true, onSelect: deleteSelected },
  ]);

  async function embedImages(html: string): Promise<string> {
    const filenames = new Set<string>();
    for (const m of html.matchAll(/<img[^>]+src="([^"]+)"/g)) {
      filenames.add(m[1]);
    }
    if (filenames.size === 0) return html;

    const imagesDir = await api.getImagesDir().catch(() => null);
    if (!imagesDir) return html;

    const replacements = new Map<string, string>();
    await Promise.all(
      [...filenames].map(async (filename) => {
        try {
          const assetUrl = convertFileSrc(`${imagesDir}/${filename}`);
          const res = await fetch(assetUrl);
          const blob = await res.blob();
          const dataUrl = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result as string);
            reader.onerror = reject;
            reader.readAsDataURL(blob);
          });
          replacements.set(filename, dataUrl);
        } catch {
          // the image is unavailable — the original src is left as is
        }
      })
    );

    let out = html;
    for (const [filename, dataUrl] of replacements) {
      out = out.split(`src="${filename}"`).join(`src="${dataUrl}"`);
    }
    return out;
  }

  function exportHtmlDocument(title: string, bodyHtml: string): string {
    const escapedTitle = title.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    return `<!DOCTYPE html>
<html lang="${i18n.lang}">
<head>
<meta charset="UTF-8">
<title>${escapedTitle}</title>
<style>
  body { max-width: 780px; margin: 2rem auto; padding: 0 1.5rem; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; line-height: 1.6; color: #1a1a1a; }
  h1, h2, h3 { line-height: 1.3; }
  img { max-width: 100%; }
  pre { background: #f4f4f5; padding: 0.75rem 1rem; border-radius: 6px; overflow-x: auto; }
  code { background: #f4f4f5; padding: 0.15em 0.4em; border-radius: 4px; }
  pre code { background: none; padding: 0; }
  blockquote { border-left: 3px solid #d4d4d8; margin-left: 0; padding-left: 1rem; color: #52525b; }
  table { border-collapse: collapse; width: 100%; }
  th, td { border: 1px solid #d4d4d8; padding: 0.4rem 0.6rem; text-align: left; }
  a.wikilink { color: #6366f1; text-decoration: none; }
  @media print { body { margin: 0; } }
</style>
</head>
<body>
<h1>${escapedTitle}</h1>
${bodyHtml}
</body>
</html>
`;
  }

  async function exportNoteAsHtml() {
    if (!selected) return;
    exporting = true;
    try {
      const rendered = renderMarkdown(editContent);
      const withImages = await embedImages(rendered);
      const html = exportHtmlDocument(editTitle || selected.title, withImages);
      const path = await saveDialog({
        defaultPath: `${(editTitle || selected.title || t("Без названия")).replace(/[/\\:*?"<>|]/g, "_")}.html`,
        filters: [{ name: "HTML", extensions: ["html"] }],
      });
      if (!path) return;
      await api.exportNoteHtml(path, html);
    } finally {
      exporting = false;
    }
  }

  async function openRevisions() {
    if (!selectedId) return;
    await flushPendingSave();
    revisionsOpen = true;
    viewingRevisionId = null;
    revisions = await api.getNoteRevisions(selectedId).catch(() => []);
  }

  function closeRevisions() {
    revisionsOpen = false;
    viewingRevisionId = null;
  }

  async function viewRevision(rev: NoteRevision) {
    viewingRevisionId = rev.id;
    viewingRevisionContent = await api.getNoteRevisionContent(rev.id).catch(() => "");
  }

  async function restoreRevision(rev: NoteRevision) {
    if (!selectedId) return;
    if (!confirm(t("Восстановить эту версию? Текущий текст тоже сохранится в версиях."))) return;
    revisionsBusy = true;
    try {
      const updated = await api.restoreNoteRevision(rev.id);
      editContent = updated.content;
      suppressNextContentSave = true;
      await noteStore.load();
      revisionsOpen = false;
    } finally {
      revisionsBusy = false;
    }
  }

  onMount(() => {
    // Reopen the last note (v0.9.79), but only once the list has arrived and only
    // if nothing is selected yet: opening the section from global search or from
    // "today's note" sets selectedId first, and those requests must win over a
    // remembered one. A note deleted since last time simply does not open.
    noteStore.load().then(() => {
      if (selectedId) return;
      const saved = loadUiState().noteId;
      const note = saved ? noteStore.notes.find(n => n.id === saved) : undefined;
      if (note) selectNote(note);
    });
    taskStore.load();
    pinnedStore.load();
    // Capability detection: with AI turned off the "Suggest links" button is hidden
    api.getSettings().then(s => aiEnabled = s.ai_provider !== "none").catch(() => {});
    const unlisteners: UnlistenFn[] = [];
    (async () => {
      unlisteners.push(await listen<{ note_id: string; titles: string[]; error: string | null }>("ai-links", (e) => {
        linkSuggesting = false;
        linkSuggestions = { noteId: e.payload.note_id, titles: e.payload.titles, error: e.payload.error };
      }));
      unlisteners.push(await listen<{ request_id: string; result: string | null; error: string | null }>("ai-selection-result", (e) => {
        if (e.payload.request_id !== selectionRequestId) return; // a reply to an already closed or superseded request
        selectionBusy = false;
        selectionResult = { requestId: e.payload.request_id, text: e.payload.result ?? "", error: e.payload.error };
      }));
      unlisteners.push(await listen<{ request_id: string; result: string | null; error: string | null }>("ai-note-summary", (e) => {
        if (e.payload.request_id !== summaryRequestId) return;
        summarizing = false;
        summaryResult = { requestId: e.payload.request_id, text: e.payload.result ?? "", error: e.payload.error };
      }));
      unlisteners.push(await listen<{ request_id: string; items: string[]; error: string | null }>("ai-extract-tasks", (e) => {
        if (e.payload.request_id !== extractRequestId) return;
        extractingTasks = false;
        extractedTasks = {
          requestId: e.payload.request_id,
          // ticked by default: the usual case is accepting nearly everything rather
          // than picking items one by one
          items: e.payload.items.map((title) => ({ id: crypto.randomUUID(), title, checked: true })),
          error: e.payload.error,
        };
      }));
    })();
    return () => unlisteners.forEach(u => u());
  });
</script>

<svelte:window onkeydown={onZenKeydown} />

<div class="notes card">
  <!-- The notes list -->
  <div class="list-pane">
    <div class="list-head">
      <button class="btn-primary btn-sm" style="width:100%;" onclick={newNote}>{t("+ Новая заметка")}</button>
      <button class="btn-ghost btn-sm" style="width:100%;" onclick={openDailyNote}><Icon name="calendar" size={12} /> {t("Сегодня")}</button>
      <div class="seg seg-list">
        <button class:active={listSubView === "notes"} onclick={() => listSubView = "notes"}>{t("Заметки")}</button>
        <button class:active={listSubView === "trash"} onclick={() => { listSubView = "trash"; noteStore.loadDeleted(); }}>{t("Корзина")}</button>
      </div>
      <input class="filter-input" bind:value={noteFilter} placeholder={t("Поиск...")} />
      <div class="filter-row">
        <select bind:value={filterTag} class="filter-select">
          <option value="">{t("Все теги")}</option>
          {#each allTags as t}
            <option value={t}>#{t}</option>
          {/each}
        </select>
        <select bind:value={filterProjectId} class="filter-select">
          <option value="">{t("Все проекты")}</option>
          {#each projectStore.active as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
      </div>
    </div>

    {#if selectedNoteIds.size > 0}
      <div class="bulk-notes-bar">
        <span class="bulk-notes-count">{t("{n} выбрано", { n: selectedNoteIds.size })}</span>
        <select bind:value={bulkNotesProjectId} disabled={bulkNotesBusy} title={t("Перенести в проект")}>
          <option value="" disabled selected>{t("В проект…")}</option>
          <option value="none">{t("Без проекта")}</option>
          {#each projectStore.active as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
        {#if bulkNotesProjectId}
          <button class="btn-sm" disabled={bulkNotesBusy} onclick={bulkMoveNotesToProject}>{t("Перенести")}</button>
        {/if}
        <button class="btn-sm btn-danger" disabled={bulkNotesBusy} onclick={bulkDeleteNotes}>{t("Удалить")}</button>
        <span style="flex:1;"></span>
        <button class="btn-icon" title={t("Снять выбор")} onclick={clearNoteSelection}>✕</button>
      </div>
    {/if}

    {#if listSubView === "trash"}
      <div class="empty-hint trash-hint">
        {t("🗑 Удалённые заметки. Восстановить можно в любой момент, пока не нажато «Удалить навсегда».")}
      </div>
      {#if noteStore.deletedNotes.length === 0}
        <div class="empty">{t("Корзина пуста")}</div>
      {:else}
        <ul class="note-list">
          {#each noteStore.deletedNotes as note (note.id)}
            <li class="note-row trashed">
              <div class="note-item">
                <div class="note-title">{note.title}</div>
                <div class="note-date">{formatDate(note.updated_at)}</div>
              </div>
              <button class="btn-icon" title={t("Восстановить")} onclick={() => noteStore.restore(note.id)}>↩</button>
              <button class="btn-icon btn-danger" title={t("Удалить навсегда")} onclick={() => noteStore.purge(note.id)}>✕</button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if noteStore.notes.length === 0}
      <div class="empty">{t("Нет заметок")}</div>
    {:else if filteredNotes.length === 0}
      <div class="empty">{t("Нет заметок по фильтру")}</div>
    {:else}
      <ul class="note-list">
        {#each filteredNotes as note, i (note.id)}
          <li
            class="note-row"
            class:pinned={note.pinned}
            class:selected={selectedNoteIds.has(note.id)}
            class:kb-focused={focusedIndex === i}
            data-note-index={i}
          >
            <button class="note-item" class:active={selectedId === note.id} onclick={(e) => onNoteRowClick(e, note)}>
              <div class="note-title">{note.title}</div>
              <div class="note-date">{formatDate(note.updated_at)}</div>
            </button>
            <button
              class="pin-btn"
              class:pinned={note.pinned}
              title={note.pinned ? t("Открепить") : t("Закрепить")}
              onclick={(e) => togglePin(note, e)}
            >
              <Icon name="pin" size={13} />
            </button>
            <!-- The "quick slot" — not to be confused with pinning above. The pin
                 raises a note to the top of the list, the bolt puts it under a
                 global hotkey. Different icons and different labels precisely
                 because the buttons sit next to each other. -->
            <button
              class="slot-btn"
              class:pinned={pinnedStore.is("note", note.id)}
              title={pinnedStore.is("note", note.id) ? t("Убрать из быстрого слота") : t("В быстрый слот (Ctrl+Shift+J)")}
              onclick={(e) => { e.stopPropagation(); pinnedStore.toggle("note", note.id); }}
            >
              <Icon name="zap" size={13} />
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <!-- The editor. In zen mode the same markup becomes a fullscreen overlay via CSS
       (class:zen on .editor-pane) rather than a separate copy of the editor: two
       LiveMarkdownEditor instances over one bind:value would mean two independent
       CodeMirror states and undo histories for the very same text — the same class
       of bug that was fixed for switching between notes. -->
  <div class="editor-pane" class:zen={zenMode}>
    {#if !selected}
      <!-- The window buttons float over the top-right corner (WindowControls is
           position:fixed and transparent). With a note open .editor-head reserves
           room for them; with none, this branch is all there is, so the reservation
           has to live here too — otherwise the buttons sit on bare pane background
           and read as covered by it. -->
      <div class="empty empty-editor">{t("Выберите заметку или создайте новую")}</div>
    {:else}
      <div class="editor-head">
        <input class="title-input" bind:value={editTitle} oninput={scheduleSave} placeholder={t("Название")} />
        {#if saving}
          <span class="muted" style="font-size:11px;">{t("Сохранение…")}</span>
        {/if}
        {#if renameToast}
          <span class="rename-toast">{renameToast}</span>
        {/if}
        <!-- Six icons in a row used to sit directly under the three window buttons,
             at the same size and spacing: the two groups read as one strip and it
             was not clear which belonged to the note. Everything but zen mode now
             lives behind "…", which shows the actions as words — a hover-only
             panel was considered and rejected for the same reason the task row's
             icons were dropped in v0.9.98: it hides them from the keyboard and
             from anyone who does not happen to point at the right corner. -->
        {#if !zenMode}
          <button
            class="btn-icon note-more"
            title={t("Действия с заметкой")}
            aria-label={t("Действия с заметкой")}
            aria-haspopup="menu"
            onclick={openNoteMenu}
          >⋯</button>
        {/if}
        <!-- Zen stays outside the menu: it is the one action used while reading
             rather than while managing the note, and it has to toggle back from
             inside zen mode, where the rest of the header is hidden. -->
        <button class="btn-icon" title={zenMode ? t("Выйти из zen-режима (Esc)") : t("Zen-режим (Ctrl+Shift+Z)")} onclick={toggleZen}>
          <Icon name={zenMode ? "collapse" : "expand"} />
        </button>
      </div>

      {#if !zenMode && linkSuggestions && linkSuggestions.noteId === selectedId}
        <div class="link-suggest">
          {#if linkSuggestions.error}
            <span class="alert" style="margin:0;">{tErr(linkSuggestions.error)}</span>
          {:else if linkSuggestions.titles.length === 0}
            <span class="muted">{t("Связей не найдено")}</span>
          {:else}
            <span class="muted">{t("Связанные:")}</span>
            <!-- The loop variable is called title rather than t: a short `t` would
                 shadow the translation function inside the block. -->
            {#each linkSuggestions.titles as title (title)}
              <button class="chip link-chip" onclick={() => acceptLinkSuggestion(title)} title="{t('Добавить связь')}: [[{title}]]">
                + {title}
              </button>
            {/each}
          {/if}
          <button class="btn-icon" title={t("Закрыть")} onclick={() => linkSuggestions = null}>✕</button>
        </div>
      {/if}

      <!-- A list of rows rather than chips: the model's wording is long and almost
           always needs editing, and it did not fit into a chip. -->
      {#if !zenMode && extractedTasks}
        <div class="extracted">
          {#if extractedTasks.error}
            <span class="alert" style="margin:0;">{tErr(extractedTasks.error)}</span>
            <button class="btn-icon" title={t("Закрыть")} onclick={closeExtractedTasks}>✕</button>
          {:else if extractedTasks.items.length === 0}
            <span class="muted">{t("Задач в заметке не найдено")}</span>
            <button class="btn-icon" title={t("Закрыть")} onclick={closeExtractedTasks}>✕</button>
          {:else}
            <div class="extracted-head">
              <span class="muted">{t("Задачи из заметки:")}</span>
              <button class="btn-sm" onclick={toggleAllExtracted} disabled={creatingExtractedTask}>
                {extractedTasks.items.every((i) => i.checked) ? t("Снять все") : t("Выбрать все")}
              </button>
              <span style="flex:1;"></span>
              <button class="btn-icon" title={t("Закрыть")} onclick={closeExtractedTasks}>✕</button>
            </div>

            <ul class="extracted-list">
              {#each extractedTasks.items as item (item.id)}
                <li class="extracted-row" class:off={!item.checked}>
                  <input
                    type="checkbox"
                    checked={item.checked}
                    disabled={creatingExtractedTask}
                    onchange={() => toggleExtracted(item.id)}
                    aria-label={t("Создать эту задачу")}
                  />
                  <input
                    class="extracted-title"
                    value={item.title}
                    disabled={creatingExtractedTask}
                    oninput={(e) => setExtractedTitle(item.id, e.currentTarget.value)}
                    placeholder={t("Название задачи")}
                  />
                </li>
              {/each}
            </ul>

            <div class="extracted-foot">
              <button
                class="btn-sm btn-primary"
                disabled={creatingExtractedTask || extractedChecked.length === 0}
                onclick={createSelectedExtracted}
              >
                {t("Создать: {n}", { n: extractedChecked.length })}
              </button>
              {#if editProjectId}
                <span class="muted extracted-hint">{t("в проект заметки")}</span>
              {/if}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Meta: the task link and tags, hidden in zen mode -->
      {#if !zenMode}
        <div class="editor-meta">
          <!-- A span, not a label: the control is a button, and a label wrapping it
               would reopen the dropdown on every click of the caption.
               "" stands in for null across the component boundary — Select works in
               strings, while an unlinked note stores null. -->
          <span class="meta-label">
            {t("Задача:")}
            <span class="meta-select">
              <Select
                value={editLinkedTaskId ?? ""}
                ariaLabel={t("Задача:")}
                onChange={(v) => { editLinkedTaskId = v || null; saveMeta(); }}
                options={[
                  { value: "", label: t("— не привязана —") },
                  ...taskStore.activeTasks.map(x => ({ value: x.id, label: x.title })),
                ]}
              />
            </span>
          </span>
          {#if projectStore.projects.length > 0}
            <label class="meta-label">
              {t("Проект:")}
              <select bind:value={editProjectId} onchange={saveMeta}>
                <option value={null}>{t("— без проекта —")}</option>
                {#each projectStore.active as p (p.id)}
                  <option value={p.id}>{p.name}</option>
                {/each}
              </select>
            </label>
          {/if}
          {#if linkedTask}
            <span class="chip"><Icon name="link" size={11} /> {linkedTask.title}</span>
          {/if}
          <label class="meta-label">
            {t("Напоминание:")}
            <input type="datetime-local" bind:value={editReminderAt} onchange={saveMeta} />
            {#if editReminderAt}
              <button class="btn-icon" title={t("Убрать напоминание")} onclick={() => { editReminderAt = ""; saveMeta(); }}>✕</button>
            {/if}
          </label>

          <div class="tags">
            {#each editTags as tag (tag)}
              <span class="chip chip-tag">
                #{tag}
                <button class="tag-remove" onclick={() => removeTag(tag)}>×</button>
              </span>
            {/each}
            <input class="tag-input" bind:value={tagInput} onkeydown={onTagKeydown} placeholder={t("+ тег")} />
          </div>
        </div>
      {/if}

      <!-- The formatting toolbar: the buttons wrap the selection in markdown
           markers through editorRef, the same path the hotkeys take
           (Ctrl+B / Ctrl+I / Ctrl+Shift+K) as registered inside the editor's CM6
           keymap — one shared implementation rather than two copies. Hidden in zen
           mode along with the rest of the chrome; the hotkeys keep working there,
           so the toolbar is not needed. -->
      {#if !zenMode}
        <div class="format-toolbar">
          <button class="btn-icon" title={t("Жирный (Ctrl+B)")} onclick={() => editorRef?.formatBold()}><Icon name="bold" /></button>
          <button class="btn-icon" title={t("Курсив (Ctrl+I)")} onclick={() => editorRef?.formatItalic()}><Icon name="italic" /></button>
          <button class="btn-icon" title={t("Заголовок")} onclick={() => editorRef?.formatHeading()}><Icon name="heading" /></button>
          <button class="btn-icon" title={t("Чек-лист")} onclick={() => editorRef?.formatChecklist()}><Icon name="checklist" /></button>
          <button class="btn-icon" title={t("Нумерованный список")} onclick={() => editorRef?.formatOrderedList()}><Icon name="orderlist" /></button>
          <button class="btn-icon" title={t("Цитата")} onclick={() => editorRef?.formatQuote()}><Icon name="quote" /></button>
          <button class="btn-icon" title={t("Вики-ссылка (Ctrl+Shift+K)")} onclick={() => editorRef?.formatWikiLink()}><Icon name="wikilink" /></button>
          <button class="btn-icon" title={t("Ссылка")} onclick={() => editorRef?.formatLink()}><Icon name="link" /></button>
          <button class="btn-icon" title={t("Код")} onclick={() => editorRef?.formatCode()}><Icon name="code" /></button>
          <button class="btn-icon" title={t("Таблица")} onclick={() => editorRef?.insertTable()}><Icon name="table" /></button>
          <!-- Dictation (v0.9.65). The component renders nothing when voice input is
               unavailable, so the toolbar simply looks as it did before. -->
          <VoiceButton onText={(text) => editorRef?.insertAtCursor(text)} />
        </div>
      {/if}

      <div class="editor-body">
        {#key selectedId}
          {#await import("../lib/components/LiveMarkdownEditor.svelte") then { default: Editor }}
            <Editor
              bind:this={editorRef}
              bind:value={editContent}
              placeholder={t("Начните писать... (Markdown, чек-листы: - [ ] пункт, ссылки: [[заметка]])")}
              knownTitles={otherTitles}
              resolveExists={(t) => findByTitle(t) !== null}
              onWikiLinkClick={openWikiLink}
              onSubmitShortcut={() => {}}
              onSelectionChange={aiEnabled && !zenMode ? onEditorSelectionChange : undefined}
            />
          {/await}
        {/key}

        {#if selectionMenu && aiEnabled && !zenMode}
          <!-- position: fixed because coordsAtPos returns viewport-relative
               coordinates and .editor-body is not the only positioned ancestor in
               the tree (zen mode makes .editor-pane a fixed overlay), so fixed is
               more reliable than recomputing into the nearest relative parent's
               coordinate system. -->
          <div class="selection-menu" style="left:{selectionMenu.left}px; top:{selectionMenu.top}px;">
            {#if selectionBusy}
              <span class="muted" style="padding:4px 8px;">{t("Думаю…")}</span>
            {:else if selectionResult}
              {#if selectionResult.error}
                <span class="alert" style="margin:0; padding:4px 8px;">{tErr(selectionResult.error)}</span>
                <button class="btn-icon" title={t("Закрыть")} onclick={dismissSelectionResult}>✕</button>
              {:else}
                <div class="selection-preview">{selectionResult.text}</div>
                <button class="btn-icon" title={t("Заменить выделение")} onclick={acceptSelectionResult}>✓</button>
                <button class="btn-icon" title={t("Отмена")} onclick={dismissSelectionResult}>✕</button>
              {/if}
            {:else}
              {#each Object.entries(SELECTION_ACTION_LABELS) as [action, label] (action)}
                <button class="chip" onclick={() => runSelectionAction(action as SelectionAction)}>{label}</button>
              {/each}
              <button class="btn-icon" title={t("Закрыть")} onclick={() => selectionMenu = null}>✕</button>
            {/if}
          </div>
        {/if}
      </div>

      {#if !zenMode && backlinks.length > 0}
        <div class="backlinks">
          <span class="backlinks-label">{t("Ссылаются сюда:")}</span>
          {#each backlinks as b (b.id)}
            <button class="backlink chip" onclick={() => selectNote(b)}>{b.title}</button>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</div>

{#if noteMenu}
  <ContextMenu
    x={noteMenu.x}
    y={noteMenu.y}
    items={noteMenuItems}
    onClose={() => noteMenu = null}
  />
{/if}

{#if revisionsOpen}
  <div class="backdrop" role="presentation" onclick={closeRevisions} onkeydown={(e) => e.key === "Escape" && closeRevisions()}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="dialog card revisions-dialog" role="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">{t("Версии заметки")}</h3>

      {#if revisions.length === 0}
        <p class="muted">{t("Ещё нет сохранённых версий — они появляются при правках с интервалом от 10 минут.")}</p>
      {:else}
        <div class="revisions-body">
          <ul class="revisions-list">
            {#each revisions as rev (rev.id)}
              <li>
                <button class="revision-item" class:active={viewingRevisionId === rev.id} onclick={() => viewRevision(rev)}>
                  <span>{formatDate(rev.created_at)}</span>
                  <span class="muted" style="font-size:11px;">{t("{n} симв.", { n: rev.size })}</span>
                </button>
              </li>
            {/each}
          </ul>
          <div class="revision-preview">
            {#if viewingRevisionId}
              <pre>{viewingRevisionContent}</pre>
              <button class="btn-primary btn-sm" disabled={revisionsBusy}
                onclick={() => restoreRevision(revisions.find(r => r.id === viewingRevisionId)!)}>
                {revisionsBusy ? t("Восстановление…") : t("Восстановить")}
              </button>
            {:else}
              <span class="muted">{t("Выберите версию слева для просмотра")}</span>
            {/if}
          </div>
        </div>
      {/if}

      <div class="actions">
        <button class="btn-ghost" onclick={closeRevisions}>{t("Закрыть")}</button>
      </div>
    </div>
  </div>
{/if}

{#if summarizing || summaryResult}
  <div class="backdrop" role="presentation" onclick={closeSummary} onkeydown={(e) => e.key === "Escape" && closeSummary()}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="dialog card summary-dialog" role="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">{t("Резюме заметки")}</h3>
      {#if summarizing}
        <p class="muted">{t("Сжимаю заметку…")}</p>
      {:else if summaryResult?.error}
        <p class="alert">{tErr(summaryResult.error)}</p>
        <div class="actions">
          <button class="btn-ghost" onclick={closeSummary}>{t("Закрыть")}</button>
        </div>
      {:else if summaryResult}
        <button class="summary-text" title={t("Скопировать и закрыть")} onclick={copySummaryAndClose}>
          {summaryResult.text}
        </button>
        <p class="muted" style="font-size:11px;">{summaryCopied ? t("Скопировано ✓") : t("Клик по тексту — скопировать и закрыть")}</p>
      {/if}
    </div>
  </div>
{/if}

<style>
  .notes {
    display: flex;
    height: 100%;
    overflow: hidden;
  }

  .list-pane {
    width: 210px;
    min-width: 170px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    background: var(--bg-secondary);
  }

  .list-head {
    padding: 8px;
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  /* The toggle fills the column like the buttons above it, and the two halves
     split it evenly. .seg is inline-flex by default, so it shrank to its labels
     and sat short of the column's right edge — "Корзина" is the longer word, so
     the halves came out uneven as well. */
  .seg-list {
    display: flex;
  }

  .seg-list button {
    flex: 1;
  }

  .filter-input {
    font-size: 12px;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-primary);
    color: var(--text-primary);
    outline: none;
    width: 100%;
    box-sizing: border-box;
  }
  .filter-input:focus { border-color: var(--accent); }

  .filter-row {
    display: flex;
    gap: 4px;
  }

  .filter-select {
    font-size: 11px;
    flex: 1;
    padding: 2px 4px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-primary);
    color: var(--text-primary);
  }

  .empty-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-secondary);
    padding: 8px 4px;
  }

  /* A trashed row is not clickable — it has no editor to open, only restore
     and purge. So .note-item here is a plain div, not a button. */
  .note-row.trashed .note-item {
    flex: 1;
    min-width: 0;
    text-align: left;
    opacity: 0.75;
    padding: 6px 8px;
  }

  .note-list {
    list-style: none;
    margin: 0;
    padding: 4px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .note-row {
    display: flex;
    align-items: center;
    gap: 2px;
    border-radius: var(--radius);
  }

  .note-row.selected {
    box-shadow: inset 3px 0 0 var(--accent);
  }

  /* The keyboard cursor is an outline, not a fill: multi-select already owns the
     left bar and the open note owns the background, so a third state needs its own
     visual channel or the three become indistinguishable when they overlap. */
  .note-row.kb-focused {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .bulk-notes-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 6px 4px;
    margin-bottom: 6px;
    border-bottom: 1px solid var(--border);
  }

  .bulk-notes-count {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
  }

  .note-item {
    display: block;
    flex: 1;
    min-width: 0;
    text-align: left;
    padding: 6px 8px;
    border: none;
    border-radius: var(--radius);
    background: transparent;
  }

  .note-item:hover { background: var(--bg-hover); }

  .note-item.active {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  .note-title {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Pinning takes the second accent: it is a state of the note, not the user's
     current choice, so it must not compete with the indigo of the active row —
     which wins when a pinned note is also the selected one. */
  .note-row.pinned .note-title { color: var(--accent-secondary); font-weight: 600; }
  .note-item.active .note-title { color: var(--accent); }

  .note-date {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: 1px;
  }

  .pin-btn {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-secondary);
    opacity: 0;
  }

  .note-row:hover .pin-btn,
  .pin-btn.pinned {
    opacity: 1;
  }

  /* The quick slot. Its own class rather than .pin-btn with a modifier: the
     pinning e2e test looks for `.pin-btn` inside a row and fails on two
     matches — rightly so, these are two different functions rather than
     variants of one. The active colour is yellow rather than the accent:
     the pin sits right beside it and is active in the accent colour, so the
     same colour would blur the two together. */
  .slot-btn {
    flex-shrink: 0;
    padding: 4px;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-secondary);
    opacity: 0;
  }

  .note-row:hover .slot-btn,
  .slot-btn.pinned {
    opacity: 1;
  }

  .slot-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* The quick slot takes the second accent, the same as in Tasks: it is a state
     of the item, not the user's current selection. Was a literal #d9a441, which
     followed neither the theme nor the chosen accent. */
  .slot-btn.pinned {
    color: var(--accent-secondary);
  }

  .pin-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .pin-btn.pinned {
    color: var(--accent);
  }

  /* min-height: 0 down the whole column: each of these is a flex item whose
     default min-height: auto would let it grow to its content instead of being
     bounded by the window. The editor's scroller can only work if every ancestor
     between it and .notes is allowed to shrink. */
  .editor-pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
  }

  .empty-editor {
    margin: auto;
  }

  .editor-pane.zen {
    position: fixed;
    inset: 0;
    z-index: 200;
    background: var(--bg-primary);
    padding: 24px clamp(16px, 10vw, 160px);
  }

  .editor-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }

  .title-input {
    flex: 1;
    font-size: 15px;
    font-weight: 600;
    border: none;
    outline: none;
    background: transparent;
    padding: 4px 0;
  }
  .title-input:focus { outline: none; }

  .link-suggest {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
  }

  .link-chip {
    border: none;
    cursor: pointer;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  /* Extracted tasks: a vertical list instead of a row of chips, because the
     model's wording is long and a row shows it in full. */
  .extracted {
    padding: 6px 12px 8px;
    border-bottom: 1px solid var(--border);
  }

  .extracted-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .extracted-list {
    list-style: none;
    margin: 6px 0;
    padding: 0;
    /* the panel shares its height with the editor: a long list scrolls on its
       own rather than pushing the note's text out of the window */
    max-height: 30vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .extracted-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* Unticking dims the row, but the text stays readable and editable: the
     user may change their mind without retyping it. */
  .extracted-row.off .extracted-title {
    opacity: .5;
  }

  .extracted-title {
    flex: 1;
    min-width: 0;
    padding: 3px 6px;
    font-size: 13px;
    border: 1px solid transparent;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-primary);
  }

  .extracted-title:hover {
    border-color: var(--border);
  }

  .extracted-title:focus {
    border-color: var(--accent);
    background: var(--bg-primary);
    outline: none;
  }

  .extracted-foot {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .extracted-hint {
    font-size: 11px;
  }
  .link-chip:hover { background: color-mix(in srgb, var(--accent) 20%, transparent); }

  .selection-menu {
    position: fixed;
    z-index: 50;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px;
    max-width: 320px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-card);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
    transform: translateY(8px);
  }

  .selection-preview {
    max-height: 160px;
    overflow-y: auto;
    padding: 4px 6px;
    font-size: 13px;
    white-space: pre-wrap;
  }

  .rename-toast {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--accent);
    white-space: nowrap;
  }

  .format-toolbar {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
  }

  .editor-meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
  }

  .meta-label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .meta-label select {
    font-size: 12px;
    max-width: 200px;
    padding: 2px 6px;
  }

  /* The task link is a Select component; the project below is still a native
     <select>. Same width so the two sit level in the meta row. */
  .meta-select {
    display: block;
    width: 200px;
  }

  .tags {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1;
    min-width: 160px;
    flex-wrap: wrap;
  }

  .tag-remove {
    border: none;
    background: transparent;
    padding: 0;
    font-size: 12px;
    line-height: 1;
    color: inherit;
  }

  .tag-input {
    font-size: 12px;
    border: none;
    outline: none;
    background: transparent;
    width: 70px;
    padding: 2px 4px;
  }
  .tag-input:focus { outline: none; }

  .editor-body {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
  }

  .backlinks {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
  }

  .backlinks-label {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .backlink {
    border: none;
    cursor: pointer;
    color: var(--accent);
  }
  .backlink:hover { text-decoration: underline; }

  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 16px;
  }

  .dialog {
    width: 100%;
    max-height: 90vh;
    overflow-y: auto;
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .revisions-dialog {
    max-width: 640px;
  }

  .summary-dialog {
    max-width: 480px;
  }

  .summary-text {
    display: block;
    width: 100%;
    text-align: left;
    cursor: pointer;
    white-space: pre-wrap;
    padding: 10px 12px;
    border: none;
    border-radius: var(--radius);
    background: var(--bg-secondary);
    color: var(--text-primary);
    font: inherit;
    line-height: 1.6;
  }
  .summary-text:hover { background: var(--bg-hover); }

  .dialog-title {
    margin: 0;
    font-size: 15px;
    font-weight: 700;
  }

  .revisions-body {
    display: grid;
    grid-template-columns: 200px 1fr;
    gap: 12px;
    min-height: 280px;
  }

  .revisions-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 340px;
    overflow-y: auto;
  }

  .revision-item {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: transparent;
    cursor: pointer;
    font-size: 12px;
  }

  .revision-item.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }

  .revision-preview {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .revision-preview pre {
    flex: 1;
    margin: 0;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: auto;
    max-height: 340px;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 12px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 4px;
  }
</style>
