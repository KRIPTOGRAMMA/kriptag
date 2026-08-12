// The subtask checklist as plain text.
//
// Subtasks used to be a set of separate <input>s: arrow keys could not move
// between lines, several lines could not be selected, and pasting a list from
// the clipboard created one subtask with newlines inside it. Now the whole
// checklist is a single <textarea>, and a line prefixed with `[x] ` / `[ ] `
// means a tick. Arrow keys, selection, Ctrl+Z and pasting all work by
// themselves, because this is ordinary text rather than emulated navigation
// between inputs.
//
// The logic lives in its own module rather than in the components: vitest in
// this project covers pure ts only (vitest.config.ts) — the same approach as in
// clipboardNote.ts and guard.ts. Besides, the parser is shared by TaskModal and
// the quick slot while they save differently (a diff on "Save" versus an
// immediate write), so only parsing the text can be shared, never the write.

export type ChecklistLine = { title: string; done: boolean };

// The tick prefix. We accept `[x]`, `[X]` and `[]` with no space inside: the
// first is what the component writes, the rest are typed by hand or arrive by
// pasting markdown. Leading spaces are consumed — a pasted list often comes
// indented. A `- ` before the brackets is a markdown list as well.
const LINE_RE = /^\s*(?:[-*]\s+)?\[( |x|X)?\]\s?(.*)$/;

// The same prefix but keeping the leading part as a separate group — the editor
// needs it to know the markup's exact bounds and hide it behind a widget. Group
// 1 is what precedes the brackets (indent, list marker) and group 2 is the tick
// itself. The whole match includes the space after the brackets, if present.
export const CHECK_RE = /^(\s*(?:[-*]\s+)?)\[( |x|X)?\]\s?/;

// Parses text into checklist lines.
//
// Empty lines are dropped: in a text field they are unavoidable (the user
// presses Enter before typing anything) but cannot be a subtask. A line with no
// prefix is an unfinished subtask rather than an error: that is how pasting an
// ordinary list from the clipboard works, and demanding manual markup from the
// user would break the main scenario for the sake of formal strictness.
export function parseChecklist(raw: string): ChecklistLine[] {
  const out: ChecklistLine[] = [];
  for (const line of raw.split("\n")) {
    const m = LINE_RE.exec(line);
    const title = (m ? m[2] : line).trim();
    if (!title) continue;
    out.push({ title, done: m ? m[1] === "x" || m[1] === "X" : false });
  }
  return out;
}

// The reverse assembly: lines back into the field's text.
//
// The prefix is always written, including for unfinished items: without it there
// would be nothing to tick — one would have to type `[ ]` by hand, whereas this
// way it is enough to put an x between the brackets.
export function formatChecklist(items: ChecklistLine[]): string {
  return items.map((i) => `[${i.done ? "x" : " "}] ${i.title}`).join("\n");
}

// A caret position into a checklist line index (empty lines are not counted, as
// in parseChecklist). Needed so a click on a checkbox knows which line of text to
// change, and so the caret can be restored after the toggle.
export function lineIndexAt(raw: string, caret: number): number {
  // A non-space character is appended to the text before the caret so the current
  // line counts even when empty: a caret at the start of the field, or right after
  // Enter, would otherwise yield -1 (parseChecklist does not count empty lines) and
  // a click on a checkbox would land on the wrong line.
  const before = raw.slice(0, Math.max(0, caret));
  return parseChecklist(before + "x").length - 1;
}

// The bounds of the line containing position `caret` in the source text.
// Returns [start, end] without the newline character.
function lineBounds(raw: string, caret: number): [number, number] {
  const pos = Math.max(0, Math.min(caret, raw.length));
  const start = raw.lastIndexOf("\n", pos - 1) + 1;
  const nl = raw.indexOf("\n", pos);
  return [start, nl === -1 ? raw.length : nl];
}

// Deletes a line entirely, along with its `[ ] ` markup.
//
// The markup is hidden behind a widget, so to the user "erase the subtask" means
// erasing the visible text. That used to leave a `[ ] ` line with no text: an
// empty line with a checkbox on screen and nothing in the data (parseChecklist
// drops it). Backspace at the start of a line must remove the line itself rather
// than the invisible brackets one character at a time.
//
// The preceding newline goes with the line — otherwise an empty line is left in
// its place and the checklist accumulates gaps as it is edited. For the first
// line we delete the newline after it instead, so the second moves up.
export function removeLineAt(raw: string, caret: number): string {
  const [start, end] = lineBounds(raw, caret);
  if (start > 0) return raw.slice(0, start - 1) + raw.slice(end);
  return raw.slice(0, start) + raw.slice(end === raw.length ? end : end + 1);
}

// The start of a line's text, right after the hidden markup. Not exported: what
// callers need is the answer to "will any text remain" (emptyAfterBackspace),
// not a position in characters — with that the caller would inevitably start
// counting columns itself and drift apart from CHECK_RE.
function textStartOf(line: string): number {
  const m = CHECK_RE.exec(line);
  return m ? m[0].length : 0;
}

// Whether any text will remain in the line if Backspace is pressed with the caret
// at column `col`.
//
// This is exactly what separates "shorten a subtask" from "delete a subtask":
// the user erases it from the end, and when the last letter goes the line must go
// with it. The `[ ] ` markup does not count as text — it is hidden behind a
// widget, so the user neither sees it nor treats it as the line's content.
export function emptyAfterBackspace(line: string, col: number): boolean {
  const start = textStartOf(line);
  // The caret is inside the markup or to its left — there is nothing left to delete
  // within the line.
  const text = col > start
    ? line.slice(start, col - 1) + line.slice(col)
    : line.slice(start);
  return !text.trim();
}

// A corrupted remnant of the markup at the start of a line.
//
// Intact markup is caught by CHECK_RE; what lands here is what remains of it after
// a word or a chunk of text was deleted: `[`, `[ `, `[x`, `]`, `- [`. No widget is
// drawn for such a line (CHECK_RE did not match) and the user sees bare brackets —
// precisely what taking apart the Xiaomi Notes APK says must never be shown.
//
// The stump must be separated from the text: `[x` is a leftover tick, whereas
// `[xyz]` and `[important]` are ordinary text in brackets that the user wrote
// themselves. Without that distinction `[important] do it` would turn into
// `[ ] important] do it` and `[xyz] code` into `[ ] yz] code`: the repair would
// corrupt data instead of fixing markup.
//
// Hence a boundary (a space or the end of the line) is required after the tick,
// and the tick itself must be either empty or exactly one character from ` xX`.
const BROKEN_RE = /^(\s*(?:[-*]\s+)?)(?:\[[ xX]?\]?|\])(?=\s|$)\s?/;

// Repairs line markup corrupted by an arbitrary deletion.
//
// A custom Backspace covered one key; Ctrl+Backspace, Delete and pasting over a
// selection went past it and left stumps such as "[ ". The result is repaired
// rather than a list of keys: a line with text gets a correct prefix, and a line
// consisting of nothing but a stump becomes empty (dropEmptyLines removes it later).
export function repairChecklistMarkup(raw: string): string {
  return raw.split("\n").map((line) => {
    if (CHECK_RE.test(line)) return line;      // the markup is intact
    const m = BROKEN_RE.exec(line);
    if (!m) return line;                        // a line with no markup at all
    const rest = line.slice(m[0].length);
    return rest.trim() ? `${m[1]}[ ] ${rest}` : "";
  }).join("\n");
}

// Drops lines with no text.
//
// An empty line is either a subtask with no name (`[ ] ` after Enter) or a bare
// blank line. parseChecklist counts neither as a subtask, so they never reach the
// DB: visible on screen, absent from the data. We clean up on losing focus rather
// than as the user types — otherwise a line would vanish under the caret at exactly
// the moment they were about to type.
export function dropEmptyLines(raw: string): string {
  return formatChecklist(parseChecklist(raw));
}

// Toggles the tick on the nth non-empty line, leaving everything else as it is
// (including empty lines and any text the user has typed but not yet formatted).
// The whole new text is returned and the caller puts it into the field without
// reassembling the content from parsed lines: reassembly would lose unfinished
// empty lines and move the caret.
export function toggleLine(raw: string, index: number): string {
  const lines = raw.split("\n");
  let seen = -1;
  for (let i = 0; i < lines.length; i++) {
    const m = LINE_RE.exec(lines[i]);
    const title = (m ? m[2] : lines[i]).trim();
    if (!title) continue;
    seen++;
    if (seen !== index) continue;
    const done = m ? m[1] === "x" || m[1] === "X" : false;
    lines[i] = `[${done ? " " : "x"}] ${title}`;
    break;
  }
  return lines.join("\n");
}

// Moves the line at `index` (counted as in parseChecklist) to the end of the
// document, so a ticked subtask sinks below the ones still to do.
//
// Called only from the checkbox click, never while typing: this is a single text
// document, and reordering on every edit would slide a line out from under the
// caret mid-word. Ticking with the mouse is a deliberate, discrete act, and the
// user is looking at the list rather than typing into it.
//
// Empty and unfinished lines stay where they are — they are what the user is in
// the middle of writing, and only the moved line changes position.
export function moveLineToEnd(raw: string, index: number): string {
  const lines = raw.split("\n");
  let seen = -1;
  let from = -1;
  for (let i = 0; i < lines.length; i++) {
    const m = LINE_RE.exec(lines[i]);
    const title = (m ? m[2] : lines[i]).trim();
    if (!title) continue;
    seen++;
    if (seen === index) { from = i; break; }
  }
  if (from === -1) return raw;

  // The last line that counts as a subtask. Anything after it is a trailing empty
  // line the user is about to type into, and the moved line goes above it rather
  // than below — otherwise the caret's line would end up under the completed one.
  let lastReal = -1;
  for (let i = lines.length - 1; i >= 0; i--) {
    const m = LINE_RE.exec(lines[i]);
    if ((m ? m[2] : lines[i]).trim()) { lastReal = i; break; }
  }
  const [moved] = lines.splice(from, 1);
  // lastReal needs no adjustment: removing a line above it shifts it down by one,
  // which is exactly the position the moved line has to take. When the line was
  // already last, that lands it back where it started and the text is unchanged —
  // no special case needed for it.
  lines.splice(lastReal, 0, moved);
  return lines.join("\n");
}
