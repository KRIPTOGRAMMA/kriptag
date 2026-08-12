// Rebindable hotkeys. A combination is stored as the normalized string
// "Ctrl+Shift+KeyN" (the modifier order is fixed and the key code is
// KeyboardEvent.code rather than .key, so it does not depend on the layout). Only
// the webview hotkeys from App.svelte live here — global OS shortcuts
// (Ctrl+Shift+N/M) are not included, they use a separate mechanism
// (tauri-plugin-global-shortcut).

export interface KeybindAction {
  id: string;
  label: string;
  defaultCombo: string;
}

export const KEYBIND_ACTIONS: KeybindAction[] = [
  { id: "palette", label: "Командная палитра / поиск", defaultCombo: "Ctrl+KeyK" },
  { id: "daily_note", label: "Заметка дня", defaultCombo: "Ctrl+KeyD" },
  // The digits follow the sidebar top to bottom, so the number is the position
  // you see. They used to be handed out in the order the screens were written:
  // the graph arrived last and took Ctrl+6 while sitting fourth in the list.
  // "Сегодня" is deliberately outside the run — it is the "right now" screen
  // rather than a section counted off, and it keeps Ctrl+`.
  { id: "view_tasks", label: "Перейти: Задачи", defaultCombo: "Ctrl+Digit1" },
  { id: "view_notes", label: "Перейти: Заметки", defaultCombo: "Ctrl+Digit2" },
  { id: "view_graph", label: "Перейти: Граф заметок", defaultCombo: "Ctrl+Digit3" },
  { id: "view_dashboard", label: "Перейти: Дашборд", defaultCombo: "Ctrl+Digit4" },
  { id: "view_calendar", label: "Перейти: Календарь", defaultCombo: "Ctrl+Digit5" },
  { id: "view_settings", label: "Перейти: Настройки", defaultCombo: "Ctrl+Digit6" },
  { id: "view_today", label: "Перейти: Сегодня", defaultCombo: "Ctrl+Backquote" },
];

export type Keybinds = Record<string, string>;

export function defaultKeybinds(): Keybinds {
  const out: Keybinds = {};
  for (const a of KEYBIND_ACTIONS) out[a.id] = a.defaultCombo;
  return out;
}

export function parseKeybinds(json: string): Keybinds {
  try {
    const v = JSON.parse(json);
    if (v && typeof v === "object" && !Array.isArray(v)) return v as Keybinds;
  } catch {
    // invalid JSON or empty — we fall back to the defaults
  }
  return {};
}

// A particular action's combination: the user's override or the default.
export function comboFor(binds: Keybinds, actionId: string): string {
  const action = KEYBIND_ACTIONS.find(a => a.id === actionId);
  return binds[actionId] ?? action?.defaultCombo ?? "";
}

const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"]);

// Builds a normalized combination from a KeyboardEvent while recording a new
// binding. Returns null if only a modifier key was pressed (we await the main key).
export function comboFromEvent(e: { ctrlKey: boolean; shiftKey: boolean; altKey: boolean; code: string }): string | null {
  if (MODIFIER_CODES.has(e.code)) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");
  parts.push(e.code);
  return parts.join("+");
}

// Whether a keyboard event matches a stored combination.
export function comboMatches(combo: string, e: { ctrlKey: boolean; shiftKey: boolean; altKey: boolean; code: string }): boolean {
  const built = comboFromEvent(e);
  return built !== null && built === combo;
}

// The human-readable form for the UI: "Ctrl+Shift+KeyN" becomes "Ctrl+Shift+N".
export function formatCombo(combo: string): string {
  return combo
    .split("+")
    .map(part => {
      if (part.startsWith("Key")) return part.slice(3);
      if (part.startsWith("Digit")) return part.slice(5);
      // Codes whose name is not the character they type. Without this the hint
      // read "Ctrl+Backquote", naming the code instead of the key on the keyboard.
      if (part === "Backquote") return "`";
      return part;
    })
    .join("+");
}

// Finds actions whose resulting combination (overrides included) matches, taking
// into account the draft being checked (draftActionId/draftCombo) before it is
// saved. Returns the ids of the conflicting actions, excluding draftActionId.
export function findConflicts(binds: Keybinds, draftActionId: string, draftCombo: string): string[] {
  const conflicts: string[] = [];
  for (const action of KEYBIND_ACTIONS) {
    if (action.id === draftActionId) continue;
    if (comboFor(binds, action.id) === draftCombo) conflicts.push(action.id);
  }
  return conflicts;
}
