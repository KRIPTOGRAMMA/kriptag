<script lang="ts">
  // A live markdown editor (Obsidian-style live preview) built on CodeMirror 6.
  // One mode only: headings, bold, italics, code, lists, checkboxes and [[links]]
  // render inline right in the text, while the syntax markers (##, **, [[ ]]) are
  // visible only on the line holding the cursor — otherwise editing would be blind.
  //
  // Note: @codemirror/lang-markdown continues a list marker ("- ") by itself when
  // Enter is pressed inside a list item — standard behaviour for such editors.
  // Programmatically inserting multi-line text containing "\n" via keyboard.type()
  // (rather than paste/insertText) triggers the same logic in e2e and duplicates the
  // marker — accounted for in the fillNoteEditor e2e helper, which uses insertText.
  import { onMount, onDestroy } from "svelte";
  import { EditorState, StateField, type Extension } from "@codemirror/state";
  import {
    EditorView, Decoration, type DecorationSet, WidgetType, keymap, ViewPlugin, type ViewUpdate,
    drawSelection, dropCursor, placeholder as cmPlaceholder,
  } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
  import { markdown } from "@codemirror/lang-markdown";
  import { syntaxTree } from "@codemirror/language";
  import {
    autocompletion, completionKeymap,
    type CompletionContext, type CompletionResult,
  } from "@codemirror/autocomplete";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { isSafeUrl } from "../urlSafety";
  import { api } from "../api/tauri";
  import { voice } from "../voice.svelte";
  import { IMAGE_RE, imageMarkdown, extImageExt, parseTableAt, serializeTable, emptyTable, type ParsedTable, type TableAlign } from "../markdown";

  // `t` is taken here by a local variable, so the translation helper is imported as `tr`.
  import { t as tr } from "../i18n.svelte";
  import * as cmd from "../editor/commands";
  let {
    value = $bindable(""),
    placeholder: placeholderText = "",
    knownTitles = [],
    resolveExists = () => false,
    onWikiLinkClick,
    onSubmitShortcut,
    onSelectionChange,
  }: {
    value: string;
    placeholder?: string;
    knownTitles?: string[];
    resolveExists?: (title: string) => boolean;
    onWikiLinkClick?: (title: string) => void;
    onSubmitShortcut?: () => void;
    onSelectionChange?: (sel: { text: string; from: number; to: number; left: number; top: number } | null) => void;
  } = $props();

  let hostEl: HTMLDivElement | undefined = $state();
  let view: EditorView | undefined;

  // knownTitles and resolveExists change reactively (the list of notes) but must not
  // recreate the editor, so they are read through a mutable wrapper that the
  // decoration plugin sees on every refresh. It is filled in the $effect below rather
  // than here, so only the prop's initial value is not captured.
  const linkCtx: {
    knownTitles: string[];
    resolveExists: (title: string) => boolean;
    onWikiLinkClick?: (title: string) => void;
  } = { knownTitles: [], resolveExists: () => false };
  $effect(() => {
    linkCtx.knownTitles = knownTitles;
    linkCtx.resolveExists = resolveExists;
    linkCtx.onWikiLinkClick = onWikiLinkClick;
    forceRebuild = true;
    // An empty transaction is the only way to trigger ViewPlugin.update() without
    // actually changing the document or the selection.
    view?.dispatch({});
  });

  class CheckboxWidget extends WidgetType {
    checked: boolean;
    pos: number;
    constructor(checked: boolean, pos: number) {
      super();
      this.checked = checked;
      this.pos = pos;
    }
    eq(other: CheckboxWidget) { return other.checked === this.checked && other.pos === this.pos; }
    toDOM() {
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = this.checked;
      box.className = "cm-task-checkbox";
      box.onmousedown = (e) => e.preventDefault(); // do not hand focus to the checkbox
      box.onclick = () => {
        if (!view) return;
        const line = view.state.doc.lineAt(this.pos);
        const text = line.text;
        const next = text.replace(/\[( |x|X)\]/, (_m, mark: string) => `[${mark === " " ? "x" : " "}]`);
        view.dispatch({ changes: { from: line.from, to: line.to, insert: next } });
      };
      return box;
    }
    ignoreEvent() { return false; }
  }

  class WikiLinkWidget extends WidgetType {
    target: string;
    label: string;
    constructor(target: string, label: string) {
      super();
      this.target = target;
      this.label = label;
    }
    eq(other: WikiLinkWidget) { return other.target === this.target && other.label === this.label; }
    toDOM() {
      const a = document.createElement("a");
      a.href = "#";
      a.className = "cm-wikilink";
      a.textContent = this.label;
      const exists = linkCtx.resolveExists(this.target);
      if (!exists) a.classList.add("missing");
      a.title = exists ? this.target : tr("Создать «{title}»", { title: this.target });
      a.onmousedown = (e) => e.preventDefault();
      a.onclick = (e) => {
        e.preventDefault();
        linkCtx.onWikiLinkClick?.(this.target);
      };
      return a;
    }
    ignoreEvent() { return false; }
  }

  // An ordinary markdown link [text](url). Unlike a wiki link it points outward, so
  // it opens in the system browser through plugin-opener rather than by internal
  // navigation. The scheme is checked: markdown may contain javascript:, data: or
  // file:, and handing those to openUrl is not acceptable.
  class MdLinkWidget extends WidgetType {
    href: string;
    label: string;
    constructor(href: string, label: string) {
      super();
      this.href = href;
      this.label = label;
    }
    eq(other: MdLinkWidget) { return other.href === this.href && other.label === this.label; }
    toDOM() {
      const a = document.createElement("a");
      a.href = "#";
      a.className = "cm-mdlink";
      a.textContent = this.label;
      const safe = isSafeUrl(this.href);
      if (!safe) a.classList.add("unsafe");
      a.title = safe ? this.href : tr("Ссылка заблокирована: {href}", { href: this.href });
      a.onmousedown = (e) => e.preventDefault();
      a.onclick = (e) => {
        e.preventDefault();
        if (safe) openUrl(this.href).catch(() => {});
      };
      return a;
    }
    ignoreEvent() { return false; }
  }

  // The absolute path to the images folder is resolved once on mount
  // (get_images_dir): convertFileSrc() requires an absolute path while the markdown
  // stores only a filename (see the paste handler below).
  let imagesDir: string | null = null;
  api.getImagesDir().then(d => { imagesDir = d; forceRebuild = true; view?.dispatch({}); }).catch(() => {});

  // Images whose markdown link has been revealed beside the rendered picture by a
  // click (by default only the picture is visible). The key is the "from:to" range of
  // the ![](...) in the document; it survives only while that range is unchanged
  // (editing text further up shifts the positions and the revealed ones return to the
  // default, which is fine for such rare clicks).
  const revealedImages = new Set<string>();

  class ImageWidget extends WidgetType {
    filename: string;
    dir: string | null;
    key: string;
    constructor(filename: string, from: number, to: number) {
      super();
      this.filename = filename;
      this.dir = imagesDir;
      this.key = `${from}:${to}`;
    }
    // imagesDir resolves asynchronously after mounting (see below), so a snapshot of
    // dir is included in eq(): otherwise CodeMirror reuses the DOM node created BEFORE
    // the path was known and src stays empty until the next edit.
    eq(other: ImageWidget) { return other.filename === this.filename && other.dir === this.dir && other.key === this.key; }
    toDOM() {
      const img = document.createElement("img");
      img.className = "cm-note-image";
      if (this.dir) {
        img.src = convertFileSrc(`${this.dir}/${this.filename}`);
      }
      img.alt = this.filename;
      img.onerror = () => img.classList.add("broken");
      img.title = tr("Клик — показать/скрыть ссылку");
      img.onmousedown = (e) => e.preventDefault();
      img.onclick = () => {
        if (revealedImages.has(this.key)) revealedImages.delete(this.key);
        else revealedImages.add(this.key);
        forceRebuild = true;
        view?.dispatch({});
      };
      return img;
    }
    ignoreEvent() { return false; }
  }

  // The table is the only widget that represents a multi-line block as a single DOM
  // structure: a click-to-edit overlay on top of a real <table> rather than mere
  // syntax highlighting (it does not fit the mark-decoration pattern used for
  // headings and bold — it needs a genuine 2D cell layout). Edits to cells accumulate
  // in this.table (mutated in place) and are serialized back into markdown by a single
  // view.dispatch on blur/Tab/Enter, not on every keystroke: otherwise each letter
  // would rebuild the whole widget and reset focus and the caret in a contenteditable
  // cell.
  // The cell to focus after the next rebuild of the widget: Tab/Enter in a cell
  // commits the edit, CM6 synchronously rebuilds the table's DOM, and the old node
  // references go stale along with the closed-over cellsGrid()/headRow/tbody. toDOM()
  // reads and clears this flag right after building the new DOM structure. It is
  // cross-widget rather than per-instance because the "next" widget is literally a
  // different JS object.
  let pendingTableFocus: { rowIndex: number; colIndex: number } | null = null;

  class TableWidget extends WidgetType {
    table: ParsedTable;
    constructor(table: ParsedTable) {
      super();
      this.table = table;
    }
    // eq() compares only the table's content, not its range: CodeMirror reuses the
    // widget's old DOM node (and, crucially, the JS instance itself) on any document
    // edit where the new TableWidget turns out to be "equal" to the old one. The
    // [from, to) range is therefore NOT stored in instance fields — there it would go
    // stale after the very first edit that recreates the widget at new positions while
    // the old JS object is reused. Instead commit() always locates the block's current
    // position afresh via view.posAtDOM(wrap), at the moment of the commit itself.
    eq(other: TableWidget) {
      return JSON.stringify(other.table) === JSON.stringify(this.table);
    }
    commit(wrap: HTMLElement, next: ParsedTable) {
      if (!view || !wrap.isConnected) return;
      // Edits in different cells (clicking the "+ row" button, moving with Tab) can
      // trigger a blur of the old cell and a click of the new command almost
      // simultaneously, and both try to commit the same widget DOM structure — already
      // stale by the time of the second call. wrap.isConnected above filters out most
      // cases, but CM6 may detach the node synchronously while posAtDOM/dispatch below
      // is running (reentrantly, from inside its own DOM update cycle). So we catch the
      // exception instead of trying to predict every race in advance: a stale commit is
      // a no-op by definition, not something worth fixing more forcefully at the cost of
      // more fragile synchronization logic.
      try {
        const from = view.posAtDOM(wrap);
        const line = view.state.doc.lineAt(from);
        const parsed = parseTableAt(view.state.doc.toString(), line.number);
        if (!parsed) return; // the document no longer starts with a table here — do not commit blindly
        const to = view.state.doc.line(parsed.endLine).to;
        const md = serializeTable(next);
        view.dispatch({ changes: { from: line.from, to, insert: md } });
      } catch {
        // A race during the widget's DOM rebuild: there is nothing left to commit, safe to ignore.
      }
    }
    toDOM() {
      const wrap = document.createElement("div");
      wrap.className = "cm-table-wrap";
      const table = document.createElement("table");
      table.className = "cm-table";
      wrap.appendChild(table);

      const alignStyle = (a: TableAlign) => a ? `text-align:${a};` : "";

      const thead = document.createElement("thead");
      const headRow = document.createElement("tr");
      this.table.header.forEach((text, c) => {
        const th = document.createElement("th");
        th.contentEditable = "true";
        th.textContent = text;
        th.style.cssText = alignStyle(this.table.align[c]);
        wireCell(th, 0, c);
        headRow.appendChild(th);
      });
      thead.appendChild(headRow);
      table.appendChild(thead);

      const tbody = document.createElement("tbody");
      this.table.rows.forEach((row, r) => {
        const tr = document.createElement("tr");
        row.forEach((text, c) => {
          const td = document.createElement("td");
          td.contentEditable = "true";
          td.textContent = text;
          td.style.cssText = alignStyle(this.table.align[c]);
          wireCell(td, r + 1, c);
          tr.appendChild(td);
        });
        tbody.appendChild(tr);
      });
      table.appendChild(tbody);

      const toolbar = document.createElement("div");
      toolbar.className = "cm-table-toolbar";
      const addRowBtn = document.createElement("button");
      addRowBtn.textContent = tr("+ строка");
      addRowBtn.onmousedown = (e) => e.preventDefault();
      addRowBtn.onclick = () => {
        const next: ParsedTable = { ...this.table, rows: [...this.table.rows, this.table.header.map(() => "")] };
        this.commit(wrap, next);
      };
      const addColBtn = document.createElement("button");
      addColBtn.textContent = tr("+ столбец");
      addColBtn.onmousedown = (e) => e.preventDefault();
      addColBtn.onclick = () => {
        const next: ParsedTable = {
          header: [...this.table.header, tr("Колонка {n}", { n: this.table.header.length + 1 })],
          align: [...this.table.align, null],
          rows: this.table.rows.map(r => [...r, ""]),
        };
        this.commit(wrap, next);
      };
      toolbar.appendChild(addRowBtn);
      toolbar.appendChild(addColBtn);
      wrap.appendChild(toolbar);

      // rowIndex 0 is the header, 1..N the body (table.rows[rowIndex-1])
      const self = this;
      // Tab/Enter commit explicitly and then move focus themselves, which also fires a
      // blur on the old cell (focus has left it) and calls commitFromDom() again on the
      // already-detached wrap of that same old DOM tree. wrap.isConnected in
      // TableWidget.commit() filters out some of these, but a repeat call can land
      // exactly while CM6 is still synchronously rebuilding the DOM after the first
      // commit (a reentrant dispatch). It is simpler and more reliable never to commit
      // twice from one and the same build of the widget.
      let committedOnce = false;
      function cellsGrid(): HTMLElement[][] {
        const headCells = Array.from(headRow.children) as HTMLElement[];
        const bodyRows = Array.from(tbody.children).map(tr => Array.from(tr.children) as HTMLElement[]);
        return [headCells, ...bodyRows];
      }
      function readCellText(el: HTMLElement): string {
        return el.textContent ?? "";
      }
      function commitFromDom() {
        if (committedOnce) return;
        committedOnce = true;
        const grid = cellsGrid();
        const header = grid[0].map(readCellText);
        const rows = grid.slice(1).map(r => r.map(readCellText));
        self.commit(wrap, { header, align: self.table.align, rows });
      }
      function focusCell(rowIndex: number, colIndex: number) {
        const grid = cellsGrid();
        const row = grid[Math.max(0, Math.min(grid.length - 1, rowIndex))];
        if (!row) return;
        const cell = row[Math.max(0, Math.min(row.length - 1, colIndex))];
        cell?.focus();
        // Put the caret at the end of the cell's text, otherwise focus lands before it.
        if (cell) {
          const range = document.createRange();
          range.selectNodeContents(cell);
          range.collapse(false);
          const sel = window.getSelection();
          sel?.removeAllRanges();
          sel?.addRange(range);
        }
      }
      function wireCell(el: HTMLElement, rowIndex: number, colIndex: number) {
        el.onblur = () => commitFromDom();
        el.onkeydown = (e) => {
          // Stop the event from reaching the CM6 keymap: Mod-b and friends must not
          // apply inside a table cell, which is its content rather than the editor's
          // document.
          e.stopPropagation();
          // Ctrl/Cmd+A: contenteditable="false" on the widget's wrapper does NOT create
          // a separate edit host for the Selection API in Chromium — a native select-all
          // inside a nested contenteditable="true" still selects the whole CM6 contentDOM
          // (verified by hand: after Ctrl+A in a cell window.getSelection() returned the
          // entire document's text). So "select everything in this cell" is implemented
          // ourselves through the Range/Selection API rather than left to the browser.
          if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
            e.preventDefault();
            const range = document.createRange();
            range.selectNodeContents(el);
            const sel = window.getSelection();
            sel?.removeAllRanges();
            sel?.addRange(range);
            return;
          }
          if (e.key === "Tab") {
            e.preventDefault();
            const grid = cellsGrid();
            const rowLen = grid[rowIndex]?.length ?? 0;
            let target: { rowIndex: number; colIndex: number };
            if (e.shiftKey) {
              target = colIndex > 0
                ? { rowIndex, colIndex: colIndex - 1 }
                : { rowIndex: rowIndex - 1, colIndex: (grid[rowIndex - 1]?.length ?? 1) - 1 };
            } else {
              target = colIndex < rowLen - 1
                ? { rowIndex, colIndex: colIndex + 1 }
                : { rowIndex: rowIndex + 1, colIndex: 0 };
            }
            // commitFromDom() rebuilds the table's DOM synchronously inside
            // view.dispatch, so we record the focus target beforehand rather than
            // calling focusCell() right after: by then headRow/tbody may already point
            // at removed nodes of the old widget.
            pendingTableFocus = target;
            commitFromDom();
          } else if (e.key === "Enter") {
            e.preventDefault();
            pendingTableFocus = { rowIndex: rowIndex + 1, colIndex };
            commitFromDom();
          } else if (e.key === "Escape") {
            (el as HTMLElement).blur();
          }
        };
      }

      // If the previous cell (in the old, already removed widget) requested focus after
      // the commit, we honour that here, in the freshly built DOM.
      if (pendingTableFocus) {
        const target = pendingTableFocus;
        pendingTableFocus = null;
        queueMicrotask(() => focusCell(target.rowIndex, target.colIndex));
      }

      return wrap;
    }
    // ignoreEvent(event) === true means "CodeMirror, do not touch this event at all"
    // (see eventBelongsToEditor in @codemirror/view — when true CM6 runs none of its
    // handlers or keymaps on it). This is needed for clicks on cells and the
    // +row/+column buttons, otherwise CM6 intercepts mousedown and makes it impossible
    // to actually click inside the widget.
    ignoreEvent(event: Event) {
      return event.type === "mousedown" || event.type === "click" || event.type === "keydown" || event.type === "blur";
    }
  }

  // Collects the Lezer ranges of code (FencedCode, InlineCode) so inline styles and
  // wiki links are not applied inside them.
  function codeRanges(state: EditorState): Set<number> {
    const set = new Set<number>();
    let depth = 0;
    syntaxTree(state).iterate({
      from: 0,
      to: state.doc.length,
      enter: (node) => {
        if (node.name === "FencedCode" || node.name === "InlineCode") {
          depth++;
          for (let p = node.from; p < node.to; p++) set.add(p);
          return false;
        }
        return undefined;
      },
    });
    return set;
  }

  function inCode(pos: number, set: Set<number>): boolean {
    return set.has(pos);
  }

  // Tables are block decorations (block: true), and CodeMirror forbids block
  // decorations from a ViewPlugin source (a dynamic facet): "Block decorations may not
  // be specified via plugins". So tables are built by a separate StateField (a static
  // source) rather than alongside the rest of the live preview in livePreviewPlugin.
  // Being a block widget rather than a single-line mark decoration, it does not need
  // hiding "only while focused" — just as ImageWidget does not check hasFocus. A cursor
  // inside the table's line range (by selection, not tied to hasFocus) shows the raw
  // markdown for textual diffing and copying.
  function buildTableDecorations(state: EditorState): DecorationSet {
    const cursorLine = state.doc.lineAt(state.selection.main.head).number;
    const codePositions = codeRanges(state);
    const items: { from: number; to: number; deco: Decoration }[] = [];
    const docText = state.doc.toString();

    for (let i = 1; i <= state.doc.lines; i++) {
      const line = state.doc.line(i);
      if (inCode(line.from, codePositions) || !line.text.includes("|")) continue;
      const parsed = parseTableAt(docText, i);
      if (!parsed) continue;
      const lastLineNum = parsed.endLine;
      const cursorInBlock = cursorLine >= i && cursorLine <= lastLineNum;
      if (!cursorInBlock) {
        const blockFrom = line.from;
        const blockTo = state.doc.line(lastLineNum).to;
        items.push({
          from: blockFrom, to: blockTo,
          deco: Decoration.replace({ widget: new TableWidget(parsed.table), block: true }),
        });
      }
      i = lastLineNum; // the loop's next iteration (i++) continues right after the table
    }

    return Decoration.set(items.map(it => it.deco.range(it.from, it.to)), true);
  }

  const tableField = StateField.define<DecorationSet>({
    create(state) { return buildTableDecorations(state); },
    update(deco, tr) {
      return tr.docChanged || tr.selection ? buildTableDecorations(tr.state) : deco;
    },
    provide: f => EditorView.decorations.from(f),
  });

  // Builds the decoration set for the whole document: the line with the cursor shows
  // raw markdown (but only while the editor is actually focused — otherwise, after a
  // programmatic value swap or a resync, a cursor on line 1 would hide the widgets
  // forever in single-line notes), and the rest show the rendered view. Tables are not
  // included here — see tableField above.
  function buildDecorations(state: EditorState, hasFocus: boolean): DecorationSet {
    const cursorLine = hasFocus ? state.doc.lineAt(state.selection.main.head).number : -1;
    const codePositions = codeRanges(state);
    const items: { from: number; to: number; deco: Decoration }[] = [];

    for (let i = 1; i <= state.doc.lines; i++) {
      const line = state.doc.line(i);
      const raw = i === cursorLine;
      const text = line.text;

      // Headings: the whole line is tagged with a size class and the '#' marker hidden
      const hLevel = text.startsWith("#") ? /^#{1,6}/.exec(text)?.[0].length ?? 0 : 0;
      if (hLevel > 0) {
        items.push({
          from: line.from, to: line.from,
          deco: Decoration.line({ class: `cm-h cm-h${hLevel}` }),
        });
        if (!raw) {
          items.push({
            from: line.from, to: line.from + hLevel + 1,
            deco: Decoration.replace({}),
          });
        }
      }

      // Checkboxes: "- [ ] " / "- [x] " become a widget
      const cbMatch = /^(\s*[-*+]\s+)\[( |x|X)\]/.exec(text);
      if (cbMatch) {
        const markStart = line.from + cbMatch[1].length;
        const markEnd = markStart + 3;
        const checked = cbMatch[2].toLowerCase() === "x";
        items.push({
          from: markStart, to: markEnd,
          deco: Decoration.replace({ widget: new CheckboxWidget(checked, line.from) }),
        });
      }

      // Images ![alt](filename), NOT inside code. By default only the rendered picture
      // is visible and the markdown link is hidden; clicking the picture reveals the
      // link beside it (revealedImages) and clicking again hides it. The picture is a
      // Decoration.widget (side: 1) placed right after the text rather than a replace:
      // the link itself is separately hidden or shown by a Decoration.replace over the
      // same range.
      for (const m of text.matchAll(IMAGE_RE)) {
        const from = line.from + m.index!;
        const to = from + m[0].length;
        if (inCode(from, codePositions)) continue;
        const filename = m[2].trim();
        if (!filename) continue;
        const key = `${from}:${to}`;
        if (!revealedImages.has(key)) {
          items.push({ from, to, deco: Decoration.replace({}) });
        }
        items.push({
          from: to, to,
          deco: Decoration.widget({ widget: new ImageWidget(filename, from, to), side: 1 }),
        });
      }

      if (!raw) {
        const lineStart = line.from;

        // Bold **text**, but not inside code
        if (!inCode(lineStart, codePositions)) {
          for (const m of text.matchAll(/\*\*([^*\n]+)\*\*/g)) {
            const from = lineStart + m.index!;
            const to = from + m[0].length;
            items.push({ from, to: from + 2, deco: Decoration.replace({}) });
            items.push({ from: from + 2, to: to - 2, deco: Decoration.mark({ class: "cm-strong" }) });
            items.push({ from: to - 2, to, deco: Decoration.replace({}) });
          }
          // Italics *text*/_text_, but not inside code
          for (const m of text.matchAll(/(?<!\*)\*([^*\n]+)\*(?!\*)|(?<!_)_([^_\n]+)_(?!_)/g)) {
            const from = lineStart + m.index!;
            const to = from + m[0].length;
            items.push({ from, to: from + 1, deco: Decoration.replace({}) });
            items.push({ from: from + 1, to: to - 1, deco: Decoration.mark({ class: "cm-em" }) });
            items.push({ from: to - 1, to, deco: Decoration.replace({}) });
          }
          // Inline code `code`
          for (const m of text.matchAll(/`([^`\n]+)`/g)) {
            const from = lineStart + m.index!;
            const to = from + m[0].length;
            items.push({ from, to: from + 1, deco: Decoration.replace({}) });
            items.push({ from: from + 1, to: to - 1, deco: Decoration.mark({ class: "cm-code" }) });
            items.push({ from: to - 1, to, deco: Decoration.replace({}) });
          }
          // Ordinary links [text](url), NOT inside code. Images ![alt](file) are
          // excluded by a negative lookbehind: they have their own ImageWidget above,
          // or one range would receive two replacements.
          for (const m of text.matchAll(/(?<!!)\[([^\[\]\n]+)\]\(([^()\s]+)\)/g)) {
            const from = lineStart + m.index!;
            const to = from + m[0].length;
            if (inCode(from, codePositions)) continue;
            const label = m[1].trim();
            const href = m[2].trim();
            if (!label || !href) continue;
            items.push({
              from, to,
              deco: Decoration.replace({ widget: new MdLinkWidget(href, label) }),
            });
          }
          // Wiki links [[target]] / [[target|label]], NOT inside code
          for (const m of text.matchAll(/\[\[([^\[\]|]+)(?:\|([^\[\]]+))?\]\]/g)) {
            const from = lineStart + m.index!;
            const to = from + m[0].length;
            if (inCode(from, codePositions)) continue;
            const target = m[1].trim();
            const label = (m[2] ?? m[1]).trim();
            if (!target) continue;
            items.push({
              from, to,
              deco: Decoration.replace({ widget: new WikiLinkWidget(target, label) }),
            });
          }
        }
      }
    }

    // Block constructs come from the Lezer tree rather than line-by-line regexes:
    // quotes and ordered lists span multiple lines and nest inside one another, and
    // parsing that with per-line regexes is wrong by construction. The tree has already
    // resolved the nesting; all that remains is tagging the lines with classes.
    syntaxTree(state).iterate({
      from: 0,
      to: state.doc.length,
      enter: (node) => {
        // FencedCode -> CodeText: a monospace background
        if (node.name === "FencedCode") {
          let child = node.node.firstChild;
          while (child) {
            if (child.name === "CodeText") {
              items.push({
                from: child.from, to: child.to,
                deco: Decoration.mark({ class: "cm-code" }),
              });
            }
            child = child.nextSibling;
          }
          return false;
        }

        // A quote: a class on every line of the block (the vertical rule and the indent
        // are drawn by CSS). The '>' marker is hidden, but only on lines without the
        // cursor, or it could never be deleted.
        if (node.name === "Blockquote") {
          const first = state.doc.lineAt(node.from).number;
          const last = state.doc.lineAt(node.to).number;
          for (let n = first; n <= last; n++) {
            const line = state.doc.line(n);
            items.push({
              from: line.from, to: line.from,
              deco: Decoration.line({ class: "cm-quote" }),
            });
            if (n === cursorLine) continue;
            // '> ' at the start of a line (possibly indented for nested quotes)
            const mark = /^\s*>\s?/.exec(line.text);
            if (mark && mark[0].length > 0) {
              items.push({
                from: line.from, to: line.from + mark[0].length,
                deco: Decoration.replace({}),
              });
            }
          }
          // return undefined: nested Blockquote/OrderedList inside a quote must be
          // processed as well.
          return undefined;
        }

        // An ordered list: a class on the line for the indent. The number itself is
        // NOT hidden — unlike '>' and '#', a digit carries meaning (the user sees and
        // edits the numbering), so it must not be replaced by a widget.
        if (node.name === "OrderedList") {
          let child = node.node.firstChild;
          while (child) {
            if (child.name === "ListItem") {
              const line = state.doc.lineAt(child.from);
              items.push({
                from: line.from, to: line.from,
                deco: Decoration.line({ class: "cm-ol-item" }),
              });
            }
            child = child.nextSibling;
          }
          return undefined;
        }

        return undefined;
      },
    });

    return Decoration.set(
      items.map(it => it.deco.range(it.from, it.to)),
      true,
    );
  }

  // A ViewPlugin rather than a StateField: the raw-versus-rendered decision depends on
  // view.hasFocus, which a StateField cannot see at all (it has no access to EditorView).
  const livePreviewPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(v: EditorView) {
        this.decorations = buildDecorations(v.state, v.hasFocus);
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.selectionSet || u.focusChanged || forceRebuild) {
          this.decorations = buildDecorations(u.state, u.view.hasFocus);
          forceRebuild = false;
        }
      }
    },
    { decorations: v => v.decorations },
  );
  // A flag meaning "the external knownTitles/resolveExists have changed":
  // ViewPlugin.update does not run by itself on reactive props, only on CM events, so
  // we trigger it via view.dispatch (an empty transaction still calls update).
  let forceRebuild = false;

  function wikiLinkCompletion(context: CompletionContext): CompletionResult | null {
    const word = context.matchBefore(/\[\[[^\[\]]*/);
    if (!word) return null;
    const query = word.text.slice(2).toLowerCase();
    const options = linkCtx.knownTitles
      .filter(t => t.toLowerCase().includes(query))
      .slice(0, 8)
      .map(t => ({ label: t, apply: `${t}]]` }));
    if (options.length === 0) return null;
    return { from: word.from + 2, options, filter: false };
  }

  const theme = EditorView.theme({
    "&": { height: "100%", fontSize: "13px" },
    ".cm-scroller": { fontFamily: "inherit", lineHeight: "1.6", overflow: "auto" },
    ".cm-content": { padding: "12px 14px" },
    "&.cm-focused": { outline: "none" },
    ".cm-h": { fontWeight: "600" },
    ".cm-h1": { fontSize: "1.5em" },
    ".cm-h2": { fontSize: "1.3em" },
    ".cm-h3": { fontSize: "1.15em" },
    ".cm-h4, .cm-h5, .cm-h6": { fontSize: "1.05em" },
    ".cm-strong": { fontWeight: "700" },
    ".cm-em": { fontStyle: "italic" },
    ".cm-code": {
      fontFamily: "monospace",
      background: "var(--bg-secondary)",
      padding: "1px 4px",
      borderRadius: "4px",
      fontSize: "0.9em",
    },
    // The second accent: this is a jump between notes, not a control. Indigo
    // inside body text would read as an ordinary interface link.
    ".cm-wikilink": {
      textDecoration: "none",
      borderBottom: "1px solid color-mix(in srgb, var(--accent-secondary) 45%, transparent)",
      color: "var(--accent-secondary)",
      cursor: "pointer",
    },
    ".cm-wikilink.missing": {
      color: "var(--text-secondary)",
      borderBottomStyle: "dashed",
    },
    // An external link, visually distinct from a wiki link:
    // a wiki link leads inside the app, this one outward into the browser.
    ".cm-mdlink": {
      textDecoration: "underline",
      textDecorationStyle: "solid",
      color: "var(--accent)",
      cursor: "pointer",
    },
    // A blocked scheme (javascript:/data:/file:): it is visible that the link
    // is dead rather than that "the click did not work".
    ".cm-mdlink.unsafe": {
      color: "var(--danger)",
      textDecorationStyle: "wavy",
      cursor: "not-allowed",
    },
    // A quote: a vertical rule on the left plus muted text
    ".cm-quote": {
      borderLeft: "3px solid color-mix(in srgb, var(--accent) 35%, transparent)",
      paddingLeft: "10px",
      color: "var(--text-secondary)",
      fontStyle: "italic",
    },
    // An ordered list: the indent only. The number stays visible —
    // it is part of the text rather than markup.
    ".cm-ol-item": { paddingLeft: "8px" },
    ".cm-task-checkbox": { marginRight: "4px", cursor: "pointer", verticalAlign: "middle" },
    ".cm-placeholder": { color: "var(--text-secondary)" },
    ".cm-note-image": {
      display: "block",
      maxWidth: "100%",
      marginTop: "4px",
      borderRadius: "6px",
    },
    ".cm-note-image.broken": {
      display: "inline-block",
      minWidth: "80px",
      minHeight: "40px",
      background: "var(--bg-secondary)",
      border: "1px dashed var(--border)",
    },
    ".cm-table-wrap": {
      margin: "6px 0",
    },
    ".cm-table": {
      borderCollapse: "collapse",
      width: "auto",
      maxWidth: "100%",
      fontSize: "0.95em",
    },
    ".cm-table th, .cm-table td": {
      border: "1px solid var(--border)",
      padding: "4px 8px",
      minWidth: "60px",
      outline: "none",
    },
    ".cm-table th": {
      background: "var(--bg-secondary)",
      fontWeight: "600",
    },
    ".cm-table td:focus, .cm-table th:focus": {
      boxShadow: "inset 0 0 0 1.5px var(--accent)",
    },
    ".cm-table-toolbar": {
      display: "flex",
      gap: "6px",
      marginTop: "4px",
    },
    ".cm-table-toolbar button": {
      fontSize: "11px",
      padding: "2px 8px",
      border: "1px solid var(--border)",
      borderRadius: "4px",
      background: "var(--bg-secondary)",
      color: "var(--text-secondary)",
      cursor: "pointer",
    },
    ".cm-table-toolbar button:hover": {
      color: "var(--text-primary)",
      borderColor: "var(--accent)",
    },
  });

  // Pasting an image from the clipboard: we intercept the paste when the clipboard's
  // files include an image/*, save it through save_note_image and insert ![](name) at
  // the cursor instead of the default paste of text or nothing.
  function fileToBase64(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result));
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(file);
    });
  }

  async function handleImagePaste(ev: ClipboardEvent, v: EditorView): Promise<boolean> {
    const items = ev.clipboardData?.items;
    // items.length === 0 means the DOM saw nothing: WebKitGTK on Linux does not pass
    // images through ClipboardEvent at all, even when the clipboard really holds an
    // image/png (types and items both arrive empty). Distinguished from "the clipboard
    // genuinely holds only text", where items is non-empty and merely has a type not
    // starting with "image/". Only an empty items warrants the native fallback.
    if (!items || items.length === 0) {
      return pasteImageFromClipboard(ev, v);
    }
    const imageItem = Array.from(items).find(it => it.type.startsWith("image/"));
    if (!imageItem) return false;
    const file = imageItem.getAsFile();
    if (!file) return false;

    ev.preventDefault();
    try {
      const dataUrl = await fileToBase64(file);
      const ext = extImageExt(file.type || file.name || "png");
      const filename = await api.saveNoteImage(dataUrl, ext);
      const markdown = imageMarkdown(filename);
      const pos = v.state.selection.main.head;
      v.dispatch({
        changes: { from: pos, insert: markdown },
        selection: { anchor: pos + markdown.length },
      });
    } catch {
      // The save failed (disk, permissions, junk in the clipboard) — insert nothing.
    }
    return true;
  }

  // WebKitGTK on Linux (including under Wayland/Hyprland) does not pass images through
  // ClipboardEvent.clipboardData: the DOM paste event arrives empty (types: [],
  // items: []) even when the clipboard really holds an image/png. Verified by hand with
  // `wl-paste --list-types`, which reports image/png while the DOM event still gives
  // items: []. This is a limitation of WebKitGTK itself rather than of this code. We
  // work around it through native clipboard access: tauri-plugin-clipboard-manager
  // reads the clipboard through the GTK API, bypassing the DOM, but returns raw RGBA
  // (Image.rgba() plus size()) rather than PNG — so we encode the PNG ourselves via a
  // canvas (toBlob), there being no ready PNG encoder in JS without extra libraries.
  async function rgbaToPngDataUrl(rgba: Uint8Array, width: number, height: number): Promise<string> {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0);
    return new Promise((resolve, reject) => {
      canvas.toBlob(blob => {
        if (!blob) { reject(new Error("toBlob failed")); return; }
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result));
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(blob);
      }, "image/png");
    });
  }

  // Returns true only if an image was actually inserted: the paste handler above uses
  // that to decide whether to call ev.preventDefault(), since up to this point the
  // clipboard may have held text that should paste normally, without interception.
  async function pasteImageFromClipboard(ev: ClipboardEvent, v: EditorView): Promise<boolean> {
    try {
      const { readImage } = await import("@tauri-apps/plugin-clipboard-manager");
      const image = await readImage();
      const [rgba, size] = await Promise.all([image.rgba(), image.size()]);
      ev.preventDefault();
      const dataUrl = await rgbaToPngDataUrl(rgba, size.width, size.height);
      const filename = await api.saveNoteImage(dataUrl, "png");
      const markdown = imageMarkdown(filename);
      const pos = v.state.selection.main.head;
      v.dispatch({
        changes: { from: pos, insert: markdown },
        selection: { anchor: pos + markdown.length },
      });
      return true;
    } catch {
      // The clipboard holds no image (text or nothing) or the plugin is unavailable —
      // we quietly skip, and an ordinary Ctrl+V for text passes through as is
      // (preventDefault has not been called on this path if readImage() failed earlier).
      return false;
    }
  }

  onMount(() => {
    if (!hostEl) return;
    const extensions: Extension[] = [
      history(),
      drawSelection(),
      dropCursor(),
      markdown(),
      livePreviewPlugin,
      tableField,
      autocompletion({ override: [wikiLinkCompletion] }),
      keymap.of([
        {
          key: "Mod-Enter",
          run: () => { onSubmitShortcut?.(); return true; },
        },
        { key: "Mod-b", run: () => { formatBold(); return true; } },
        { key: "Mod-i", run: () => { formatItalic(); return true; } },
        { key: "Mod-Shift-k", run: () => { formatWikiLink(); return true; } },
        // Dictation (v0.9.66). The handler is async while `run` has to answer
        // synchronously, so the promise is left running and the key is reported as
        // handled straight away — otherwise the combination would fall through to
        // defaultKeymap while the recording starts.
        {
          key: "Mod-Shift-d",
          run: () => {
            void dictate();
            return true;
          },
        },
        ...historyKeymap,
        ...completionKeymap,
        ...defaultKeymap,
      ]),
      theme,
      EditorView.lineWrapping,
      cmPlaceholder(placeholderText),
      EditorView.domEventHandlers({
        paste: (event, v) => {
          void handleImagePaste(event, v);
          // Returning from domEventHandlers does not cancel the paste by itself —
          // that is done by event.preventDefault() inside handleImagePaste,
          // and only when an image was actually found in the clipboard.
          return false;
        },
      }),
      EditorView.updateListener.of(update => {
        if (update.docChanged) {
          value = update.state.doc.toString();
        }
        if (update.docChanged || update.selectionSet) {
          reportSelection(update.view);
        }
      }),
    ];

    view = new EditorView({
      state: EditorState.create({ doc: value, extensions }),
      parent: hostEl,
    });
  });

  onDestroy(() => view?.destroy());

  // External changes to value (switching notes, inserting from the composer or AI):
  // the document is synced only when it has genuinely diverged from the state.
  $effect(() => {
    const v = value;
    if (view && view.state.doc.toString() !== v) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: v },
      });
    }
  });

  export function focus() {
    view?.focus();
  }

  // AI on a selection: we tell the parent about a non-empty selection so it can
  // show a floating action menu beside it. The coordinates are already page-level
  // (coordsAtPos returns a viewport-relative rect of the view itself, not of the
  // host), so the parent need not recompute anything.
  function reportSelection(v: EditorView) {
    if (!onSelectionChange) return;
    const range = v.state.selection.main;
    if (range.empty) {
      onSelectionChange(null);
      return;
    }
    const text = v.state.sliceDoc(range.from, range.to);
    const coords = v.coordsAtPos(range.head);
    if (!coords) {
      onSelectionChange(null);
      return;
    }
    onSelectionChange({ text, from: range.from, to: range.to, left: coords.left, top: coords.top });
  }

  // Replaces a range with the result of an AI action on the selection. from/to are
  // the positions as of when the menu opened; the caller must await the model's
  // reply before calling this method, but the document may have changed meanwhile.
  // If the range no longer matches the current selection (the user clicked or
  // typed), the edit is still applied at the same numeric positions — safe, since
  // this is a final confirmed user action rather than a background operation.
  // Dictation by Ctrl+Shift+D: the same recording the microphone button drives, so
  // the key and the button never disagree about whether one is running.
  //
  // Nothing happens when voice input is unavailable — the key is then simply not
  // ours, exactly as the button is absent rather than disabled.
  async function dictate() {
    if (!(await voice.ensureChecked())) return;
    const text = await voice.toggle();
    if (text) insertAtCursor(text);
  }

  // The public surface is unchanged: same names, same signatures, so EditorExports
  // in Notes.svelte and every caller keep working. Only the bodies moved — into
  // lib/editor/commands.ts, where vitest can reach them.
  export function replaceRange(from: number, to: number, text: string) {
    if (view) cmd.replaceRange(view, from, to, text);
  }

  export function insertAtCursor(text: string) {
    if (view) cmd.insertAtCursor(view, text);
  }

  export function formatBold() { if (view) cmd.wrapSelection(view, "**", "**"); }
  export function formatItalic() { if (view) cmd.wrapSelection(view, "*", "*"); }
  export function formatCode() { if (view) cmd.wrapSelection(view, "`", "`"); }
  export function formatHeading() { if (view) cmd.toggleLinePrefix(view, "## "); }
  export function formatChecklist() { if (view) cmd.toggleLinePrefix(view, "- [ ] "); }
  export function formatWikiLink() { if (view) cmd.wrapSelection(view, "[[", "]]"); }
  export function formatQuote() { if (view) cmd.toggleLinePrefix(view, "> "); }
  export function formatOrderedList() { if (view) cmd.toggleOrderedList(view); }
  // The template is the only translated string here, so it stays in the component
  // and is passed in — commands.ts has no i18n dependency.
  export function formatLink() { if (view) cmd.insertLink(view, tr("[текст](url)")); }
  export function insertTable() { if (view) cmd.insertTable(view); }
</script>

<div class="cm-host" bind:this={hostEl}></div>

<style>
  .cm-host {
    flex: 1;
    display: flex;
    overflow: hidden;
    /* A flex item defaults to min-height: auto and refuses to shrink below its
       content, so this box wants to grow to the full height of the note. Chromium
       bounds it anyway because of the overflow: hidden above, which is why the
       e2e suite never saw a problem — but that is engine behaviour, not a rule to
       rely on, and the app runs in WebKitGTK where a long note could not be
       scrolled: the host grew to the document's height, CodeMirror sized
       .cm-scroller to the host, and there was nothing left to scroll while
       .editor-body clipped the rest away. */
    min-height: 0;
  }
  .cm-host :global(.cm-editor) {
    width: 100%;
  }
</style>
