// A mirror of src-tauri/src/core/task.rs::Task.
// The Rust side does not use #[serde(rename_all)], so the field names in JSON
// match the struct's field names one for one.

// A status is the id of a row in the statuses table (the same trick the category
// uses) rather than a fixed set. Todo/InProgress/Done/Archived remain reserved ids
// (see StatusInfo.is_reserved) because business logic is tied to them (Done ->
// hidden+completed_at, InProgress -> time tracking), but custom intermediate
// statuses for the kanban board can appear alongside them.
export type TaskStatus = string;
export type Priority = "Low" | "Medium" | "High" | "Critical";
// A category is the id of a row in the categories table (user-defined) rather
// than a fixed set. The name and colour come through CategoryInfo.
export type Category = string;
export type RecurrenceUnit = "Minutes" | "Hours" | "Days" | "Weeks";

export interface CategoryInfo {
  id: string;
  name: string;
  color: string;
  position: number;
}

export interface StatusInfo {
  id: string;
  name: string;
  color: string;
  position: number;
  is_reserved: boolean;
}

export type Recurrence =
  | "None"
  | "Hourly"
  | "Daily"
  | "Weekly"
  | { Custom: [number, RecurrenceUnit] }
  | { Weekdays: number }; // a bitmask: bit 0 = Monday ... bit 6 = Sunday

export interface Subtask {
  id: string;
  task_id: string;
  title: string;
  done: boolean;
  position: number;
}

export interface Task {
  id: string;
  title: string;
  description: string | null;
  status: TaskStatus;
  priority: Priority;
  category: Category;
  deadline: string | null; // RFC3339, arrives as a string through JSON
  tags: string[];
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  recurrence: Recurrence;
  hidden: boolean;
  deleted_at: string | null; // soft deletion — non-null means it is in the Trash
  project_id: string | null;
  scheduled_at: string | null; // time block: the start (RFC3339)
  scheduled_mins: number | null; // time block: the duration
  sort_order: number; // manual ordering in the list (drag)
  subtasks: Subtask[];
  // Dependencies: the open blockers of this task. An empty array means the task is
  // free. A blocker in the Trash does not appear here (it does not block), but the
  // link is alive and returns with it on restore.
  blocked_by: Blocker[];
}

// A blocker with its title, so "Blocked by X" can be shown without an extra
// request for the name.
export interface Blocker {
  id: string;
  title: string;
}

export interface Project {
  id: string;
  name: string;
  color: string;
  target_date: string | null;
  archived: boolean;
  created_at: string;
  task_total: number;
  task_done: number;
  goal_tasks: number | null; // goal: tasks per period
  goal_mins: number | null; // goal: minutes of time blocks per period
  goal_period: "week" | "month";
  goal_done_tasks: number; // progress for the current period
  goal_done_mins: number;
}

export interface GoalSnapshot {
  id: string;
  project_id: string;
  period_key: string;
  goal_tasks: number | null;
  goal_mins: number | null;
  done_tasks: number;
  done_mins: number;
  recorded_at: string;
}

export interface UpdateProjectPayload {
  name?: string;
  color?: string;
  target_date?: string; // an empty string clears the date
  archived?: boolean;
  goal_tasks?: number; // 0 clears the goal
  goal_mins?: number; // 0 clears the goal
  goal_period?: "week" | "month";
}

export interface CreateTaskPayload {
  title: string;
  description: string | null;
  status: TaskStatus;
  priority: Priority;
  category: Category;
  deadline: string | null;
  tags: string[];
  recurrence: Recurrence;
  project_id?: string | null;
}

export interface UpdateTaskPayload {
  title?: string;
  description?: string;
  status?: TaskStatus;
  priority?: Priority;
  category?: Category;
  deadline?: string;
  tags?: string[];
  recurrence?: Recurrence;
  project_id?: string; // an empty string detaches it from the project
  scheduled_at?: string; // an empty string clears the time block
  scheduled_mins?: number;
}

export interface Note {
  id: string;
  title: string;
  content: string;
  tags: string[];
  linked_task_id: string | null;
  project_id: string | null;
  pinned: boolean;
  created_at: string;
  updated_at: string;
  reminder_at: string | null;
}

export interface NoteSnippet {
  item: Note;
  snippet: string;
}

export interface NoteRevision {
  id: string;
  created_at: string;
  size: number;
}

export interface TaskSnippet {
  item: Task;
  snippet: string;
}

export interface CreateNotePayload {
  title: string;
  content: string;
  tags?: string[];
  linked_task_id?: string | null;
  project_id?: string | null;
}

export interface UpdateNotePayload {
  title?: string;
  content?: string;
  tags?: string[];
  linked_task_id?: string | null;
  project_id?: string | null;
  pinned?: boolean;
  reminder_at?: string | null;
}

export interface AppSettings {
  ai_provider: "none" | "local" | "openai" | "anthropic";
  openai_key: string;
  openai_model: string;
  anthropic_key: string;
  anthropic_model: string;
  idle_threshold_secs: number;
  log_interval_secs: number;
  work_mode: "Light" | "Study" | "Focus";
  onboarding_complete: boolean;
  deadline_warn_hours: number;
  deadline_warn_minutes: number;
  idle_notify_min_mins: number;
  pomodoro_work_mins: number;
  pomodoro_break_mins: number;
  nudge_after_mins: number;
  theme_mode: "light" | "dark" | "system";
  color_accent: string;
  color_accent_secondary: string; // the second accent (the .btn-primary gradient); empty means equal to color_accent
  color_bg: string;
  color_bg_secondary: string; // the sidebar and second-plane surfaces (--bg-secondary)
  color_bg_hover: string; // the hover fill of rows and ghost buttons (--bg-hover)
  color_bg_card: string; // cards, buttons, task rows (--bg-card)
  color_text_secondary: string; // captions, dates, counters (--text-secondary)
  color_text: string;
  color_border: string;
  quiet_until: string; // RFC3339; empty means off; a distant date means indefinite
  context_notifications: boolean;
  ai_fallback: boolean;
  openai_in_keyring: boolean;
  anthropic_in_keyring: boolean;
  custom_theme_presets: string; // JSON [{name, colors:{color_*}}] — user-saved colour sets
  app_category_rules: string; // JSON [{pattern, category}]
  app_limits: string;         // JSON [{category, daily_mins}] — 0 or absence means no limit
  auto_backup_dir: string;    // empty means automatic backup is off
  auto_backup_keep: number;   // how many copies to keep (minimum 1)
  // Read-only: written by the backup loop, ignored by save_settings. See the
  // field comment in src-tauri/src/commands/settings.rs.
  last_auto_backup: string;   // RFC3339 of the last successful automatic backup
  // Also read-only. "<rfc3339>\t<message>", cleared as soon as a run succeeds.
  last_auto_backup_error: string;
  morning_digest_time: string; // "HH:MM", empty means off
  show_subtasks_expanded: boolean; // subtasks visible in the list without a click
  keybinds: string; // JSON {action_id: combo}; a missing key means the action's default
  // The same for global quick-capture hotkeys, under a separate key: they use a
  // different mechanism (OS-level registration) and a combination may fail to apply.
  global_keybinds: string;
  focus_mode_auto: boolean; // automatically pause notifications during pomodoro work or a time block
  track_domains: boolean; // breaking browser time down by site; off by default
  language: string; // "ru" | "en"; empty means detect from the system locale
  history_cleanup_months: number; // completed items older than N months go to the Trash automatically; 0 = off
}

// An action launched by a global hotkey. The list comes from the backend
// (list_global_actions), which also registers them, so the copies must not drift.
export interface GlobalAction {
  id: string;
  label: string;
  default_combo: string;
}

export interface AppCategoryRule {
  pattern: string;
  category: Category;
}

export interface AppLimit {
  category: Category;
  daily_mins: number;
}

export interface ChecklistTemplate {
  id: string;
  name: string;
  items: string[];
}

export interface SmartListFilter {
  category: string | null;
  priority: string | null;
  tag: string | null;
  has_deadline: boolean | null;
}

export interface SmartList {
  id: string;
  name: string;
  filter: SmartListFilter;
  position: number;
}

export interface NotificationEntry {
  id: string;
  kind: string;
  title: string;
  body: string;
  created_at: string;
  read_at: string | null;
  entity_type: string | null;
  entity_id: string | null;
}

export interface DayCompletion {
  id: string;
  title: string;
}

export interface Routine {
  id: string;
  title: string;
  days_mask: number;
  start_mins: number;
  duration_mins: number;
  active: boolean;
}

export interface RoutineBlock {
  title: string;
  start_mins: number;
  duration_mins: number;
}

export interface ActiveSession {
  task_id: string;
  title: string;
  started_at: string;
  elapsed_secs: number;
}

// Which engine a model feeds. Mirrors ModelKind in commands/model.rs, where the
// variants are serialized in lowercase.
export type ModelKind = "llm" | "whisper";

export interface ModelOption {
  id: string;
  name: string;
  url: string;
  size_bytes: number;
  description: string;
  ram_gb: number;
  recommended: boolean;
  kind: ModelKind;
}

// The quick-capture window's mode. "clipboard" is the same note form but
// pre-filled with text from the clipboard; it mirrors normalize_quick_mode in
// lib.rs, where an unknown mode folds into "task". "pinned" is the only mode that
// creates no record but opens an existing one for text editing.
export type QuickMode = "task" | "note" | "clipboard" | "pinned";

// The contents of the quick slot. `text` is a task's description or a note's
// content: the window edits text only, hence a single field.
export interface PinnedItem {
  kind: "task" | "note";
  id: string;
  title: string;
  text: string;
  // A task's checklist travels with the slot. For a note it is always empty — the
  // field is always present so callers need not branch on whether it exists.
  subtasks: Subtask[];
}

// Idle time inside a planned time block: the plan comes from the task, the actual
// from activity monitoring.
export interface BlockIdle {
  task_id: string;
  task_title: string;
  planned_mins: number;
  idle_mins: number;
  active_mins: number;
}

// What a backup archive holds, read before the import is confirmed (v0.9.92).
// The file dialog shows only a timestamp in the name, and the import is
// irreversible — losing_* is the number that actually stops a mistake: how much
// of the CURRENT database is newer than the snapshot and would be discarded.
export interface ImportPreview {
  tasks: number;
  notes: number;
  // The same counts for the database being replaced — the dialog shows the
  // difference, not a bare number.
  current_tasks: number;
  current_notes: number;
  newest: string;        // RFC3339 of the newest row in the archive; empty if it has none
  losing_tasks: number;
  losing_notes: number;
}
