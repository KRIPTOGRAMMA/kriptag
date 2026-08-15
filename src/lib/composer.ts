// The parser for the inline task composer. The text comes from a textarea where
// Shift+Enter adds a subtask line prefixed with "☐ ": the first ordinary non-empty
// line is the title, the remaining ordinary lines are the description, and lines
// with "☐" are subtasks. A pure function: the UI inserts the prefixes, this only
// parses them.

export const SUBTASK_PREFIX = "☐ ";

export interface ComposerDraft {
  title: string;
  description: string;
  subtasks: string[];
}

/// Splits "☐" lines off from ordinary text, without deciding what the text means.
///
/// The quick-capture window needs exactly this and nothing more: it has a title
/// field of its own, so every line of its description field is description. Going
/// through parseComposer there would silently eat the first line, which is the
/// title only when one field holds both.
export function splitSubtaskLines(src: string): { text: string; subtasks: string[] } {
  const subtasks: string[] = [];
  const textLines: string[] = [];

  for (const line of (src ?? "").split("\n")) {
    if (line.trimStart().startsWith("☐")) {
      const t = line.trimStart().slice(1).trim();
      if (t) subtasks.push(t);
    } else {
      textLines.push(line);
    }
  }

  return { text: textLines.join("\n"), subtasks };
}

export function parseComposer(src: string): ComposerDraft {
  const { text, subtasks } = splitSubtaskLines(src);
  const textLines = text.split("\n");

  // The composer is one field for everything, so its first ordinary line is the
  // title and the rest is the description.
  while (textLines.length > 0 && !textLines[0].trim()) textLines.shift();
  const title = (textLines.shift() ?? "").trim();
  const description = textLines.join("\n").trim();

  return { title, description, subtasks };
}

// --- Natural language in a task's title ---
// "tomorrow 15:00 call !high @work #important" becomes title "call" plus metadata.
// The tokens are recognized only in the title line (the title from parseComposer),
// not in the description or subtasks — a natural boundary that leaves the rest of
// the text as is.
//
// Marker syntax (it does not clash with the existing #tag on tasks):
//   !marker    — priority (low/medium/high/critical plus synonyms)
//   @marker    — category (matched by category name, case-insensitively)
//   #marker    — tag (as before, simply added to tags)
//   date/time  — "tomorrow", "the day after tomorrow", "today", a weekday, "HH:MM":
//                any combination of date and time, in any order relative to the text

export interface ParsedTaskMeta {
  title: string;
  priority: "Low" | "Medium" | "High" | "Critical" | null;
  categoryQuery: string | null; // the raw word after @ — matched against categoryStore outside
  tags: string[];
  deadline: Date | null;
}

const PRIORITY_WORDS: Record<string, ParsedTaskMeta["priority"]> = {
  "низкий": "Low", "низк": "Low",
  "средний": "Medium", "средн": "Medium", "норм": "Medium", "обычный": "Medium",
  "высокий": "High", "высок": "High", "срочно": "High", "важно": "High",
  "критический": "Critical", "критично": "Critical", "критик": "Critical",
};

const WEEKDAYS: Record<string, number> = {
  "понедельник": 1, "вторник": 2, "среда": 3, "среду": 3, "четверг": 4,
  "пятница": 5, "пятницу": 5, "суббота": 6, "субботу": 6, "воскресенье": 0,
};

function matchPriority(word: string): ParsedTaskMeta["priority"] {
  const norm = word.toLowerCase().replace(/[^a-zа-яё]/gi, "");
  for (const [key, value] of Object.entries(PRIORITY_WORDS)) {
    if (norm.startsWith(key)) return value;
  }
  return null;
}

// The nearest date with this weekday (0 = Sunday), including today when it matches
// and the time has not passed yet; otherwise next week. Simplified: always the next
// occurrence, without treating "today" as a special case (the user will say "today").
function nextWeekday(from: Date, targetDow: number): Date {
  const d = new Date(from);
  const diff = (targetDow - d.getDay() + 7) % 7 || 7;
  d.setDate(d.getDate() + diff);
  return d;
}

function applyTime(d: Date, hh: number, mm: number): Date {
  const out = new Date(d);
  out.setHours(hh, mm, 0, 0);
  return out;
}

export function parseTaskText(rawTitle: string, now: Date = new Date()): ParsedTaskMeta {
  const tokens = rawTitle.split(/\s+/).filter(Boolean);
  const titleWords: string[] = [];
  let priority: ParsedTaskMeta["priority"] = null;
  let categoryQuery: string | null = null;
  const tags: string[] = [];

  let datePart: Date | null = null; // the date only (00:00), set by day words
  let timeHH: number | null = null;
  let timeMM: number | null = null;

  for (const token of tokens) {
    if (token.startsWith("!") && token.length > 1) {
      const p = matchPriority(token.slice(1));
      if (p) { priority = p; continue; }
    }
    if (token.startsWith("@") && token.length > 1) {
      categoryQuery = token.slice(1);
      continue;
    }
    if (token.startsWith("#") && token.length > 1) {
      tags.push(token.slice(1));
      continue;
    }

    const timeMatch = /^(\d{1,2}):(\d{2})$/.exec(token);
    if (timeMatch) {
      const hh = Number(timeMatch[1]);
      const mm = Number(timeMatch[2]);
      if (hh <= 23 && mm <= 59) {
        timeHH = hh;
        timeMM = mm;
        continue;
      }
    }

    const lower = token.toLowerCase().replace(/[.,!?]+$/, "");
    if (lower === "сегодня") {
      datePart = applyTime(now, 0, 0);
      continue;
    }
    if (lower === "завтра") {
      datePart = applyTime(now, 0, 0);
      datePart.setDate(datePart.getDate() + 1);
      continue;
    }
    if (lower === "послезавтра") {
      datePart = applyTime(now, 0, 0);
      datePart.setDate(datePart.getDate() + 2);
      continue;
    }
    if (lower in WEEKDAYS) {
      datePart = applyTime(nextWeekday(now, WEEKDAYS[lower]), 0, 0);
      continue;
    }

    titleWords.push(token);
  }

  let deadline: Date | null = null;
  if (datePart || timeHH !== null) {
    const base = datePart ?? new Date(now);
    deadline = timeHH !== null ? applyTime(base, timeHH, timeMM ?? 0) : base;
  }

  return {
    title: titleWords.join(" ").trim(),
    priority,
    categoryQuery,
    tags,
    deadline,
  };
}

// Matching @category against existing categories by name or id, on the same
// normalizing principle as match_category on the backend (commands/categories.rs)
// for AI classification: trim punctuation at the edges, ignore case.
export function matchCategoryQuery(
  categories: { id: string; name: string }[],
  query: string,
): string | null {
  const norm = query.trim().replace(/^[^a-zа-яё0-9]+|[^a-zа-яё0-9]+$/gi, "").toLowerCase();
  if (!norm) return null;
  const found = categories.find(
    c => c.name.toLowerCase() === norm || c.id.toLowerCase() === norm,
  );
  return found?.id ?? null;
}
