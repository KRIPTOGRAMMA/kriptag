import { invoke } from "@tauri-apps/api/core";
import type { Task, Subtask, Blocker, CreateTaskPayload, UpdateTaskPayload, Note, CreateNotePayload, UpdateNotePayload, AppSettings, Project, UpdateProjectPayload, CategoryInfo, StatusInfo, NoteSnippet, TaskSnippet, GoalSnapshot, Routine, RoutineBlock, ActiveSession, NoteRevision, ChecklistTemplate, DayCompletion, ModelOption, ModelKind, SmartList, SmartListFilter, NotificationEntry, QuickMode, BlockIdle, PinnedItem, GlobalAction, ImportPreview } from "../types";

export const api = {
  getTasks: () => invoke<Task[]>("get_tasks"),
  createTask: (task: CreateTaskPayload) => invoke<Task>("create_task", { task }),
  updateTask: (id: string, patch: UpdateTaskPayload) => invoke<Task>("update_task", { id, patch }),
  deleteTask: (id: string) => invoke<void>("delete_task", { id }),
  getDeletedTasks: () => invoke<Task[]>("get_deleted_tasks"),
  restoreTask: (id: string) => invoke<void>("restore_task", { id }),
  purgeDeletedTask: (id: string) => invoke<void>("purge_deleted_task", { id }),
  completeTask: (id: string) => invoke<Task>("complete_task", { id }),
  searchTasks: (query: string) => invoke<Task[]>("search_tasks", { query }),
  reorderTasks: (ids: string[]) => invoke<void>("reorder_tasks", { ids }),
  getProjects: () => invoke<Project[]>("get_projects"),
  createProject: (project: { name: string; color?: string; target_date?: string | null }) =>
    invoke<Project>("create_project", { project }),
  updateProject: (id: string, patch: UpdateProjectPayload) =>
    invoke<void>("update_project", { id, patch }),
  deleteProject: (id: string) => invoke<void>("delete_project", { id }),
  getGoalHistory: (projectId: string) => invoke<GoalSnapshot[]>("get_goal_history", { projectId }),
  getCategories: () => invoke<CategoryInfo[]>("get_categories"),
  createCategory: (name: string, color: string) => invoke<CategoryInfo>("create_category", { name, color }),
  updateCategory: (id: string, patch: { name?: string; color?: string }) =>
    invoke<void>("update_category", { id, patch }),
  deleteCategory: (id: string) => invoke<void>("delete_category", { id }),
  getStatuses: () => invoke<StatusInfo[]>("get_statuses"),
  createStatus: (name: string, color: string) => invoke<StatusInfo>("create_status", { name, color }),
  updateStatus: (id: string, patch: { name?: string; color?: string }) =>
    invoke<void>("update_status", { id, patch }),
  deleteStatus: (id: string) => invoke<void>("delete_status", { id }),
  recordInput: () => invoke<void>("record_input"),
  openQuickCapture: (mode: QuickMode) => invoke<void>("open_quick_capture", { mode }),
  getQuickMode: () => invoke<QuickMode>("get_quick_mode"),
  readClipboardText: () => invoke<string>("read_clipboard_text"),
  getPinnedItem: () => invoke<PinnedItem | null>("get_pinned_item"),
  setPinnedItem: (kind: "task" | "note" | null, id: string | null) =>
    invoke<void>("set_pinned_item", { kind, id }),
  aiRewrite: (taskId: string, title: string) => invoke<void>("ai_rewrite", { taskId, title }),
  aiSubtasks: (taskId: string, title: string) => invoke<void>("ai_subtasks", { taskId, title }),
  aiClassify: (taskId: string, title: string) => invoke<void>("ai_classify", { taskId, title }),
  aiPlanDay: () => invoke<void>("ai_plan_day"),
  aiWhatNow: () => invoke<void>("ai_what_now"),
  getNotes: () => invoke<Note[]>("get_notes"),
  createNote: (note: CreateNotePayload) => invoke<Note>("create_note", { note }),
  updateNote: (id: string, patch: UpdateNotePayload) => invoke<Note>("update_note", { id, patch }),
  deleteNote: (id: string) => invoke<void>("delete_note", { id }),
  getDeletedNotes: () => invoke<Note[]>("get_deleted_notes"),
  restoreNote: (id: string) => invoke<void>("restore_note", { id }),
  purgeDeletedNote: (id: string) => invoke<void>("purge_deleted_note", { id }),
  searchNotes: (query: string) => invoke<Note[]>("search_notes", { query }),
  searchNotesSnippet: (query: string) => invoke<NoteSnippet[]>("search_notes_snippet", { query }),
  searchTasksSnippet: (query: string) => invoke<TaskSnippet[]>("search_tasks_snippet", { query }),
  renameNoteLinks: (oldTitle: string, newTitle: string) =>
    invoke<number>("rename_note_links", { oldTitle, newTitle }),
  aiSuggestLinks: (noteId: string) => invoke<void>("ai_suggest_links", { noteId }),
  aiEditSelection: (requestId: string, text: string, mode: string) =>
    invoke<void>("ai_edit_selection", { requestId, text, mode }),
  aiSummarizeNote: (requestId: string, text: string) => invoke<void>("ai_summarize_note", { requestId, text }),
  aiExtractTasks: (requestId: string, text: string) => invoke<void>("ai_extract_tasks", { requestId, text }),
  aiAskNotes: (requestId: string, question: string) => invoke<void>("ai_ask_notes", { requestId, question }),
  getNoteRevisions: (noteId: string) => invoke<NoteRevision[]>("get_note_revisions", { noteId }),
  getNoteRevisionContent: (revisionId: string) => invoke<string>("get_note_revision_content", { revisionId }),
  restoreNoteRevision: (revisionId: string) => invoke<Note>("restore_note_revision", { revisionId }),
  saveNoteImage: (dataBase64: string, ext: string) => invoke<string>("save_note_image", { dataBase64, ext }),
  getImagesDir: () => invoke<string>("get_images_dir"),
  exportNotesMd: (dir: string) => invoke<number>("export_notes_md", { dir }),
  importNotesMd: (dir: string) => invoke<number>("import_notes_md", { dir }),
  exportNoteHtml: (path: string, html: string) => invoke<void>("export_note_html", { path, html }),
  getChecklistTemplates: () => invoke<ChecklistTemplate[]>("get_checklist_templates"),
  createChecklistTemplate: (name: string, items: string[]) =>
    invoke<ChecklistTemplate>("create_checklist_template", { name, items }),
  deleteChecklistTemplate: (id: string) => invoke<void>("delete_checklist_template", { id }),
  getSmartLists: () => invoke<SmartList[]>("get_smart_lists"),
  createSmartList: (name: string, filter: SmartListFilter) =>
    invoke<SmartList>("create_smart_list", { name, filter }),
  deleteSmartList: (id: string) => invoke<void>("delete_smart_list", { id }),
  getNotificationLog: () => invoke<NotificationEntry[]>("get_notification_log"),
  getUnreadNotificationCount: () => invoke<number>("get_unread_notification_count"),
  markNotificationsRead: () => invoke<void>("mark_notifications_read"),
  clearNotificationLog: () => invoke<void>("clear_notification_log"),
  getActivityByDay: () => invoke<{ date: string; minutes: number }[]>("get_activity_by_day"),
  getTaskCompletionsByDay: () => invoke<{ date: string; completed: number }[]>("get_task_completions_by_day"),
  getCategoryDistribution: () => invoke<{ category: string; count: number }[]>("get_category_distribution"),
  getBlockIdle: (date: string) => invoke<BlockIdle[]>("get_block_idle", { date }),
  getActiveIdleRatio: () =>
    invoke<{ today_active: number; today_idle: number; week_active: number; week_idle: number }>("get_active_idle_ratio"),
  getDomainUsage: (days: number) => invoke<{ domain: string; minutes: number }[]>("get_domain_usage", { days }),
  clearDomainHistory: () => invoke<number>("clear_domain_history"),
  getAppUsage: (days: number) => invoke<{ app: string; minutes: number }[]>("get_app_usage", { days }),
  // Apps with no matching rule — the input for AI classification.
  getUncategorizedApps: (days: number) =>
    invoke<{ app: string; minutes: number }[]>("get_uncategorized_apps", { days }),
  // Proposes rules; the answer arrives as an "ai-app-rules" event, like the other
  // AI commands. Nothing is written to the settings — the user confirms.
  aiSuggestAppRules: () => invoke<void>("ai_suggest_app_rules"),
  getCompletionsForDay: (date: string) => invoke<DayCompletion[]>("get_completions_for_day", { date }),
  getHourlyActivity: (days: number) =>
    invoke<{ weekday: number; hour: number; minutes: number }[]>("get_hourly_activity", { days }),
  getPomodoroState: () => invoke<{ phase: string; until: string | null }>("get_pomodoro_state"),
  pomodoroTogglePause: () => invoke<void>("pomodoro_toggle_pause"),
  pomodoroSkip: () => invoke<void>("pomodoro_skip"),
  pomodoroStart: () => invoke<void>("pomodoro_start"),
  pomodoroStop: () => invoke<void>("pomodoro_stop"),
  getPomodoroStats: () =>
    invoke<{ today: number; week: number; task_streak: number; pomodoro_streak: number }>("get_pomodoro_stats"),
  getAppCategoryTime: (days: number) =>
    invoke<{ category: string; minutes: number }[]>("get_app_category_time", { days }),
  dashboardInsight: () => invoke<void>("dashboard_insight"),
  summarizeDay: () => invoke<void>("summarize_day"),
  summarizeWeek: () => invoke<void>("summarize_week"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  // Global hotkeys. The action list comes from the backend, which also registers
  // them; there must be no second copy in TS.
  listGlobalActions: () => invoke<GlobalAction[]>("list_global_actions"),
  // The combination is validated by global-hotkey rather than by our own rules in TS.
  validateGlobalCombo: (combo: string) => invoke<void>("validate_global_combo", { combo }),
  // Applies the saved combinations without a restart; returns those that could not
  // be registered (taken by the system).
  applyGlobalHotkeys: () => invoke<string[]>("apply_global_hotkeys"),
  saveSettings: (settings: AppSettings) => invoke<void>("save_settings", { settings }),
  isWayland: () => invoke<boolean>("is_wayland"),
  getTrackingMode: () => invoke<"extended" | "basic">("get_tracking_mode"),
  getWindowTracking: () => invoke<string | null>("get_window_tracking"),
  exportData: (path: string) => invoke<void>("export", { path }),
  importData: (path: string) => invoke<void>("import", { path }),
  previewImport: (path: string) => invoke<ImportPreview>("preview_import", { path }),
  doAutoBackup: () => invoke<string>("do_auto_backup"),
  getSubtasks: (taskId: string) => invoke<Subtask[]>("get_subtasks", { taskId }),
  addSubtask: (taskId: string, title: string) => invoke<Subtask>("add_subtask", { taskId, title }),
  toggleSubtask: (id: string) => invoke<void>("toggle_subtask", { id }),
  deleteSubtask: (id: string) => invoke<void>("delete_subtask", { id }),
  renameSubtask: (id: string, title: string) => invoke<void>("rename_subtask", { id, title }),
  getTaskBlockers: (taskId: string) => invoke<Blocker[]>("get_task_blockers", { taskId }),
  addTaskDependency: (taskId: string, blockerId: string) =>
    invoke<void>("add_task_dependency", { taskId, blockerId }),
  removeTaskDependency: (taskId: string, blockerId: string) =>
    invoke<void>("remove_task_dependency", { taskId, blockerId }),
  getRoutines: () => invoke<Routine[]>("get_routines"),
  createRoutine: (routine: { title: string; days_mask: number; start_mins: number; duration_mins: number }) =>
    invoke<Routine>("create_routine", { routine }),
  updateRoutine: (id: string, patch: { title?: string; days_mask?: number; start_mins?: number; duration_mins?: number; active?: boolean }) =>
    invoke<void>("update_routine", { id, patch }),
  deleteRoutine: (id: string) => invoke<void>("delete_routine", { id }),
  startTaskTracking: (taskId: string) => invoke<ActiveSession>("start_task_tracking", { taskId }),
  stopTaskTracking: () => invoke<void>("stop_task_tracking"),
  getActiveSession: () => invoke<ActiveSession | null>("get_active_session"),
  getTaskSeconds: (taskId: string) => invoke<number>("get_task_seconds", { taskId }),
  getProjectSeconds: (projectId: string, from: string) => invoke<number>("get_project_seconds", { projectId, from }),
  // kind is optional on every model command: omitting it means the chat model,
  // which is what these calls meant before voice input existed.
  listModelOptions: (kind?: ModelKind) => invoke<ModelOption[]>("list_model_options", { kind }),
  modelStatus: (kind?: ModelKind) => invoke<{ exists: boolean; size_bytes: number }>("model_status", { kind }),
  modelPath: (kind?: ModelKind) => invoke<string>("model_path", { kind }),
  downloadModel: (url: string, kind?: ModelKind) => invoke<void>("download_model", { url, kind }),
  // Voice input. voiceAvailable is capability detection: both the model and the
  // whisper-cli binary have to exist, or the button is simply absent.
  voiceAvailable: () => invoke<boolean>("voice_available"),
  startVoiceRecording: () => invoke<void>("start_voice_recording"),
  stopVoiceRecording: () => invoke<string>("stop_voice_recording"),
  cancelVoiceRecording: () => invoke<void>("cancel_voice_recording"),
};
