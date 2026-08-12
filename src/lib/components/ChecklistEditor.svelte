<script lang="ts">
  // The subtask checklist as a single editor.
  //
  // Every subtask used to be its own <input>: arrow keys did not move the cursor
  // between lines, several lines could not be selected, and pasting a list from
  // the clipboard produced one subtask with newlines inside it.
  //
  // This version's first attempt folded the list into a <textarea> with visible
  // `[x] ` prefixes and put the checkboxes in a column beside it. Taking apart the
  // Xiaomi Notes APK (com.miui.richeditor.style.CheckboxSpan) showed it is done
  // differently there, and the difference is fundamental: the checkbox is part of
  // the line rather than a neighbouring column, and the markup is not visible to
  // the user at all (in their format it exists only in serialization,
  // `HtmlParser$CheckBoxElement` -> `<input type="checkbox">`). Visible brackets
  // are something the user can corrupt, and a column beside the text drifts out of
  // alignment on any line that wraps.
  //
  // So this uses CodeMirror with the same technique that already works in notes:
  // `Decoration.replace` hides `[x] ` and draws a real checkbox in its place,
  // inside the line. The text stays a single document, so arrow keys, selection
  // across lines, undo and pasting all come from the editor.
  //
  // We take bare CodeMirror rather than LiveMarkdownEditor: that one brings wiki
  // links, images, tables and autocompletion, none of which belong here.
  import { onMount, onDestroy } from "svelte";
  import { EditorState } from "@codemirror/state";
  import {
    EditorView, Decoration, type DecorationSet, WidgetType,
    keymap, ViewPlugin, type ViewUpdate, drawSelection, placeholder as cmPlaceholder,
  } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
  import {
    CHECK_RE, toggleLine, lineIndexAt, removeLineAt, emptyAfterBackspace,
    dropEmptyLines, repairChecklistMarkup, moveLineToEnd, parseChecklist,
  } from "../checklistText";

  type Props = {
    value: string;
    placeholder?: string;
    // The text is passed as an argument rather than only through bind:value: the
    // panel in the task list keeps it in a dictionary keyed by task id, and a
    // two-way binding to a dictionary entry is awkward there.
    onchange?: (text: string) => void;
  };

  let { value = $bindable(""), placeholder = "", onchange }: Props = $props();

  let hostEl: HTMLDivElement | undefined = $state();
  let view: EditorView | undefined;

  const SVG_NS = "http://www.w3.org/2000/svg";

  // The box as a pair of brackets rather than a full square: two vertical edges
  // with short arms top and bottom, which is what `[ ]` looks like and what the
  // markup underneath literally is. The tick sits between them.
  //
  // Coordinates are in a 14x14 viewBox, matching the rendered size 1:1, so the
  // strokes land on whole pixels instead of being resampled.
  const CHECK_PATHS: [string, string][] = [
    ["cm-sub-bracket", "M4.6 1.2 L1.6 1.2 L1.6 12.8 L4.6 12.8"],
    ["cm-sub-bracket", "M9.4 1.2 L12.4 1.2 L12.4 12.8 L9.4 12.8"],
    ["cm-sub-tick", "M4.2 7.2 L6.4 9.4 L10 4.9"],
  ];

  class CheckboxWidget extends WidgetType {
    checked: boolean;
    pos: number;
    // The line's text is part of the widget's identity, and it has to be. The DOM
    // node is a real <input type="checkbox">, so clicking it flips the browser's
    // own checked property; CodeMirror reuses any node whose widget compares equal.
    // Comparing only {checked, pos} meant that after a tick reordered the lines,
    // the line moving into a vacated offset matched the widget that used to live
    // there and inherited its flipped box. Measured: ticking one of two subtasks
    // drew BOTH boxes checked while the text and the strike-through were correct.
    text: string;
    constructor(checked: boolean, pos: number, text: string) {
      super();
      this.checked = checked;
      this.pos = pos;
      this.text = text;
    }
    eq(other: CheckboxWidget) {
      return other.checked === this.checked && other.pos === this.pos
        && other.text === this.text;
    }
    toDOM() {
      // A real <input type="checkbox"> wrapped in a <label>, with an SVG drawn on
      // top. The input keeps the semantics — a screen reader still announces a
      // checkbox and its state — while the SVG supplies the shape, which is a
      // bracket pair and cannot be made out of a box's own borders.
      //
      // SVG rather than a font glyph or an image: nothing to load, nothing to go
      // missing from a font, and the strokes take their colour from the theme
      // tokens like everything else.
      const wrap = document.createElement("label");
      wrap.className = "cm-sub-box";

      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = this.checked;
      box.className = "cm-sub-checkbox";

      const svg = document.createElementNS(SVG_NS, "svg");
      svg.setAttribute("viewBox", "0 0 14 14");
      svg.setAttribute("aria-hidden", "true");
      svg.classList.add("cm-sub-svg");
      for (const [cls, d] of CHECK_PATHS) {
        const path = document.createElementNS(SVG_NS, "path");
        path.setAttribute("d", d);
        path.classList.add(cls);
        svg.appendChild(path);
      }

      wrap.append(box, svg);
      // Focus stays in the text: clicking a checkbox must not knock the caret out
      // of the line the user is editing.
      wrap.onmousedown = (e) => e.preventDefault();
      box.onclick = () => {
        if (!view) return;
        const doc = view.state.doc.toString();
        const index = lineIndexAt(doc, this.pos);
        const toggled = toggleLine(doc, index);
        if (toggled === doc) return;
        // Ticking sinks the subtask below the ones still to do; unticking leaves it
        // where it is. Reordering happens only here, on a deliberate mouse click —
        // doing it on every edit would slide a line out from under the caret while
        // the user is typing, and this is one shared text document.
        const nowDone = parseChecklist(toggled)[index]?.done === true;
        const next = nowDone ? moveLineToEnd(toggled, index) : toggled;
        view.dispatch({ changes: { from: 0, to: doc.length, insert: next } });
      };
      return wrap;
    }
    ignoreEvent() { return false; }
  }

  // The markup is always hidden, including on the line with the cursor — unlike in
  // notes, where markers show under the caret. Here the brackets are not part of a
  // note's text but the way a tick is stored: there is no reason to show them to
  // the user, and every chance they would corrupt them.
  //
  // A checkbox is drawn on EVERY non-empty line, not only on a marked-up one.
  // Otherwise things fall out of sync: `parseChecklist` treats a line with no
  // prefix as a subtask (needed for pasting a ready-made list), so on save it
  // becomes one — yet it has no checkbox, and the user sees some lines as subtasks
  // and others as plain text. On an unmarked line the widget is inserted at the
  // start (`Decoration.widget`) rather than replacing the text.
  function buildDecos(state: EditorState): DecorationSet {
    const items: { from: number; to: number; deco: Decoration }[] = [];
    for (let n = 1; n <= state.doc.lines; n++) {
      const line = state.doc.line(n);
      const m = CHECK_RE.exec(line.text);
      if (m) {
        const done = m[2] === "x" || m[2] === "X";
        // A line decoration, not a mark over the text range: it has to survive the
        // line being empty and must not fight the replace decoration that hides the
        // markup at the same position.
        if (done) {
          items.push({
            from: line.from,
            to: line.from,
            deco: Decoration.line({ class: "cm-sub-done" }),
          });
        }
        const from = line.from + m[1].length;
        const to = from + m[0].length - m[1].length;
        items.push({
          from,
          to,
          deco: Decoration.replace({
            widget: new CheckboxWidget(done, line.from, line.text),
          }),
        });
      } else if (line.text.trim()) {
        items.push({
          from: line.from,
          to: line.from,
          deco: Decoration.widget({ widget: new CheckboxWidget(false, line.from, line.text), side: -1 }),
        });
      }
    }
    return Decoration.set(items.map((i) => i.deco.range(i.from, i.to)), true);
  }

  const checkboxPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(v: EditorView) { this.decorations = buildDecos(v.state); }
      update(u: ViewUpdate) {
        if (u.docChanged) this.decorations = buildDecos(u.state);
      }
    },
    { decorations: (v) => v.decorations },
  );

  // Enter continues the list, as in any editor with checklists. Without it a new
  // line would have no markup and there would be nothing to tick.
  function newSubtaskLine(v: EditorView): boolean {
    const { state } = v;
    const at = state.selection.main.head;
    // EVERY new line gets the markup, including one after an empty line. There
    // used to be an early return on an empty current line, so Enter there fell
    // through to defaultKeymap and produced a line with no checkbox — something a
    // checklist cannot contain, since every line here is a subtask.
    const insert = "\n[ ] ";
    v.dispatch({
      changes: { from: at, to: at, insert },
      selection: { anchor: at + insert.length },
    });
    return true;
  }

  const enterKeymap = keymap.of([
    { key: "Enter", run: newSubtaskLine },
    // Shift+Enter is bound explicitly even though defaultKeymap already routes it
    // to Enter (verified in a browser: without this line the behaviour is the
    // same). It is declared so as not to rest on an internal detail of someone
    // else's keymap: in a checklist every line is a subtask, there are no paragraph
    // breaks, so both combinations must do the same thing.
    { key: "Shift-Enter", run: newSubtaskLine },
    // Ctrl/Cmd+Enter belongs to the window rather than to the editor: in the quick
    // slot it means "save", and in the task modal too. Without an explicit binding
    // the combination fell through to defaultKeymap and inserted an empty unmarked
    // line — the list broke and the save did not fire.
    //
    // We return true (the combination is handled, so CodeMirror does nothing) but do
    // not stop the event: the keydown bubbles up to the <svelte:window> outside,
    // which is what calls submit.
    { key: "Mod-Enter", run: () => true },
  ]);

  // A Backspace that erases a subtask's last letter removes the subtask entirely.
  //
  // The `[ ] ` markup is hidden behind a widget, so to the user a line is its text.
  // Subtasks are erased from the end rather than by putting the caret at the start:
  // when the last letter disappears, the subtask must go with it. Otherwise an
  // empty line with a checkbox stays on screen (it is already gone from the data —
  // parseChecklist drops empty ones) and one more press on the invisible brackets
  // is required.
  //
  // The condition is "after this deletion no text will remain" rather than "the
  // caret is at the start of the line": the latter describes how one gets to the
  // line, the former describes what the user considers deleting a subtask.
  //
  // It only fires on a collapsed selection: Backspace over selected text must
  // delete it as usual rather than remove the line.
  const backspaceKeymap = keymap.of([
    {
      key: "Backspace",
      run(v) {
        const { state } = v;
        const sel = state.selection.main;
        if (!sel.empty) return false;
        const line = state.doc.lineAt(sel.head);
        // Text remains in the line — an ordinary character deletion.
        if (!emptyAfterBackspace(line.text, sel.head - line.from)) return false;

        // The only empty line: there is nothing to delete, or Backspace on an empty
        // field would swallow it whole for no visible reason.
        if (state.doc.lines === 1) return false;

        const doc = state.doc.toString();
        const next = removeLineAt(doc, sel.head);
        // The caret goes to the end of the previous line, as with an ordinary
        // Backspace at a line boundary (line.from - 1 is the position of its last
        // character once the newline is gone). For the first line we put it at the
        // start of the document.
        const anchor = line.number > 1 ? line.from - 1 : 0;
        v.dispatch({
          changes: { from: 0, to: doc.length, insert: next },
          selection: { anchor: Math.min(anchor, next.length) },
        });
        return true;
      },
    },
  ]);

  // No deletion may corrupt the markup.
  //
  // A custom Backspace covered only one key. Ctrl+Backspace (delete word) went past
  // it and ate the brackets from the inside, leaving a visible stump "[ " in the
  // line — precisely the markup the user must never see. Delete, Ctrl+Delete and
  // pasting over a selection do the same.
  //
  // So we fix the result rather than the keys one by one: if a change left the line
  // inconsistent (text present, markup broken), we restore it to a valid form.
  // Enumerating the combinations is pointless — there are more of them than can be
  // anticipated, and each new CodeMirror version may add its own.
  const repairMarkup = EditorState.transactionFilter.of((tr) => {
    if (!tr.docChanged) return tr;
    const text = tr.newDoc.toString();
    const fixed = repairChecklistMarkup(text);
    if (fixed === text) return tr;
    // The caret is kept in place: what is repaired lies to its left, so the shift is
    // computed from the difference in lengths up to the caret's position.
    const head = tr.newSelection.main.head;
    const delta = fixed.length - text.length;
    return [
      { changes: { from: 0, to: tr.startState.doc.length, insert: fixed },
        selection: { anchor: Math.max(0, Math.min(head + delta, fixed.length)) },
        scrollIntoView: true },
    ];
  });

  const theme = EditorView.theme({
    "&": { fontSize: "13px" },
    "&.cm-focused": { outline: "none" },
    ".cm-content": { padding: "4px 6px", fontFamily: "inherit", caretColor: "var(--text-primary)" },
    ".cm-line": { padding: "0" },
    ".cm-scroller": { fontFamily: "inherit", lineHeight: "20px" },
    // The box is drawn by hand rather than left to the platform. The native
    // checkbox is round on this GTK theme and takes its colour from the system
    // rather than from the app's tokens — the same reason the <select> popups had
    // to be replaced.
    //
    // The <label> is the visible box; the <input> inside it is kept for semantics
    // and hidden, not removed, so it still carries the state to a screen reader.
    ".cm-sub-box": {
      position: "relative",
      display: "inline-block",
      width: "14px",
      height: "14px",
      margin: "0 6px 0 0",
      flex: "0 0 auto",
      cursor: "pointer",
      verticalAlign: "-2px",
    },
    // Hidden but still focusable and still read out: opacity rather than
    // display:none, which would take it out of the accessibility tree entirely.
    ".cm-sub-checkbox": {
      position: "absolute",
      inset: "0",
      width: "100%",
      height: "100%",
      margin: "0",
      opacity: "0",
      cursor: "pointer",
    },
    ".cm-sub-svg": {
      display: "block",
      width: "14px",
      height: "14px",
      pointerEvents: "none",
    },
    ".cm-sub-bracket": {
      fill: "none",
      stroke: "var(--text-secondary)",
      strokeWidth: "1.6",
      strokeLinecap: "round",
      strokeLinejoin: "round",
      transition: "stroke 160ms ease",
    },
    ".cm-sub-box:hover .cm-sub-bracket": { stroke: "var(--accent)" },
    ".cm-sub-checkbox:focus-visible ~ .cm-sub-svg .cm-sub-bracket": {
      stroke: "var(--accent)",
    },
    // The ticked colour is set by the cm-sub-tint animation below rather than
    // declared here: with both, the static rule wins the cascade over an animation
    // that has not started, and the brackets snap to the accent before the tick is
    // drawn — the two halves of the same movement come apart.
    // The tick is dashed out by its own length so an unticked box shows nothing.
    // getTotalLength() on the rendered path measures 8.9 units; 10 is a small
    // deliberate overshoot, so the line is certainly hidden without measuring at
    // runtime.
    ".cm-sub-tick": {
      fill: "none",
      stroke: "var(--accent)",
      strokeWidth: "2",
      strokeLinecap: "round",
      strokeLinejoin: "round",
      strokeDasharray: "10",
      strokeDashoffset: "10",
    },
    // An animation, not a transition. A transition needs the element to persist
    // across the change, and this one never does: every toggle rewrites the whole
    // document, CodeMirror rebuilds the widget, and the new node arrives already
    // ticked with no previous value to move from. Measured — the node identity
    // differs before and after a click, and a transition produced no intermediate
    // frames at all. An animation runs on mount, which is exactly when the ticked
    // node appears.
    ".cm-sub-checkbox:checked ~ .cm-sub-svg .cm-sub-tick": {
      animation: "cm-sub-draw 220ms cubic-bezier(0.65, 0, 0.35, 1) forwards",
    },
    "@keyframes cm-sub-draw": {
      from: { strokeDashoffset: "10" },
      to: { strokeDashoffset: "0" },
    },
    // The brackets tint towards the accent over the same beat, so the box reads as
    // one movement rather than a line appearing next to a colour change.
    ".cm-sub-checkbox:checked ~ .cm-sub-svg .cm-sub-bracket": {
      animation: "cm-sub-tint 220ms ease forwards",
    },
    "@keyframes cm-sub-tint": {
      from: { stroke: "var(--text-secondary)" },
      to: { stroke: "var(--accent)" },
    },
    // Motion is a preference, not a given: with it off the tick still appears, it
    // just stops being drawn on.
    "@media (prefers-reduced-motion: reduce)": {
      ".cm-sub-checkbox:checked ~ .cm-sub-svg .cm-sub-tick": {
        animation: "none",
        strokeDashoffset: "0",
      },
      ".cm-sub-checkbox:checked ~ .cm-sub-svg .cm-sub-bracket": {
        animation: "none",
        stroke: "var(--accent)",
      },
      ".cm-sub-bracket": { transition: "none" },
    },
    // A completed subtask is struck through and dimmed. Both, not just the line:
    // on a dark theme a thin rule over full-contrast text is easy to miss, and the
    // dimming is what makes "done" readable at a glance down the list.
    ".cm-sub-done": {
      textDecoration: "line-through",
      color: "var(--text-secondary)",
    },
    ".cm-placeholder": { color: "var(--text-secondary)" },
  });

  onMount(() => {
    if (!hostEl) return;
    view = new EditorView({
      parent: hostEl,
      state: EditorState.create({
        doc: value,
        extensions: [
          history(),
          drawSelection(),
          checkboxPlugin,
          repairMarkup,
          // Our handlers come before defaultKeymap: that one would intercept
          // Backspace and delete a single invisible markup character instead of the
          // line.
          enterKeymap,
          backspaceKeymap,
          keymap.of([...defaultKeymap, ...historyKeymap]),
          EditorView.lineWrapping,
          cmPlaceholder(placeholder),
          theme,
          // Losing focus is the moment editing is finished, so empty lines are
          // cleaned up here rather than as the user types. Otherwise a line would
          // vanish under the caret at exactly the moment the user pressed Enter and
          // was about to type a name.
          //
          // Replacing the document dispatches an ordinary change, so the
          // updateListener below passes the cleaned text outward by itself.
          EditorView.domEventHandlers({
            blur(_e, v) {
              const doc = v.state.doc.toString();
              const cleaned = dropEmptyLines(doc);
              if (cleaned === doc) return false;
              v.dispatch({
                changes: { from: 0, to: doc.length, insert: cleaned },
              });
              return false;
            },
          }),
          EditorView.updateListener.of((u) => {
            if (!u.docChanged) return;
            value = u.state.doc.toString();
            onchange?.(value);
          }),
        ],
      }),
    });
  });

  onDestroy(() => view?.destroy());

  // An external value swap (loading the slot, applying a template). Comparing
  // against the current document is mandatory: without it the editor's own edit
  // would come back here through the bind and move the caret to the end on every
  // letter.
  $effect(() => {
    const next = value;
    if (view && next !== view.state.doc.toString()) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: next } });
    }
  });
</script>

<div class="checklist-editor" bind:this={hostEl}></div>

<style>
  .checklist-editor {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-card);
    max-height: 30vh;
    overflow-y: auto;
  }
  .checklist-editor :global(.cm-editor) {
    background: transparent;
  }
</style>
