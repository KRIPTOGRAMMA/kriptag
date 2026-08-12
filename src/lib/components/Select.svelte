<script lang="ts">
  // A dropdown that replaces a native <select>.
  //
  // Not a style preference: WebKitGTK draws a <select>'s popup with the GTK system
  // theme, and no CSS reaches inside it. `color-scheme: dark` on :root fixes this
  // in Chromium — which is why e2e never showed it — but not in the webview the app
  // actually runs in, so on a dark theme the list opened white.
  //
  // Only where a list is read while choosing (the task modal, a note's linked task).
  // The filters and Settings keep their native selects: a native control is better
  // when it works, and outside a dark panel the mismatch does not read as a fault.
  import { tick } from "svelte";
  import { placeDropdown } from "../menuPos";

  export interface Option {
    value: string;
    label: string;
  }

  /** Widest the list may get before option text starts truncating. */
  const MAX_LIST_W = 420;
  /** Keeps the list off the window edge; mirrors EDGE_GAP in menuPos.ts. */
  const EDGE_GAP = 4;

  type Props = {
    value: string;
    options: Option[];
    onChange: (value: string) => void;
    // Shown when nothing matches `value` — the "Add blocker..." case, where the
    // control is an action rather than a display of current state.
    placeholder?: string;
    disabled?: boolean;
    title?: string;
    ariaLabel?: string;
  };

  let { value, options, onChange, placeholder, disabled, title, ariaLabel }: Props = $props();

  let btnEl: HTMLButtonElement | undefined = $state();
  let listEl: HTMLElement | undefined = $state();
  let open = $state(false);
  let pos = $state<{ x: number; y: number; maxH: number } | null>(null);
  let width = $state(0);
  // A ceiling for the list, so one long task title cannot stretch it across the
  // screen. Recomputed against the window in place(): on a narrow one even
  // MAX_LIST_W would overhang, and the option text truncates to whatever is left.
  let maxW = $state(MAX_LIST_W);
  // Which option the keyboard is on. Separate from `value`: moving through the
  // list must not commit until Enter, or arrow keys would fire a change per step.
  let active = $state(-1);

  const selected = $derived(options.find(o => o.value === value));
  const label = $derived(selected?.label ?? placeholder ?? "");

  async function show() {
    if (disabled) return;
    open = true;
    active = options.findIndex(o => o.value === value);
    // Measured after the list exists: its height depends on the options, and the
    // room for it depends on where the control sits. Rendered off-screen for one
    // frame so the measurement does not flash in the wrong place.
    await tick();
    await place();
    // Focus deliberately stays on the trigger button rather than moving into the
    // list: the list is visibility:hidden for the frame between mount and
    // measurement, and a hidden element silently refuses focus. The button
    // forwards the keys instead, so arrows and Escape work either way.
  }

  async function place() {
    if (!btnEl || !listEl) return;
    const r = btnEl.getBoundingClientRect();
    width = r.width;
    // The ceiling goes on first and is awaited: it is what keeps the measurement
    // below honest, and reading the width in the same synchronous block would
    // return the unconstrained one, before the style reached the DOM. Never
    // narrower than the trigger — a list inside its own button looks broken.
    maxW = Math.max(r.width, Math.min(MAX_LIST_W, window.innerWidth - 2 * EDGE_GAP));
    await tick();
    if (!btnEl || !listEl) return;
    // The list's own width, not the trigger's: options are capped by max-width
    // above but can still be wider than the control that opened them, and the
    // clamp has to know the real number or the surplus hangs off the window.
    pos = placeDropdown(
      r,
      listEl.scrollHeight,
      window.innerWidth,
      window.innerHeight,
      undefined,
      listEl.getBoundingClientRect().width,
    );
  }

  function close() {
    open = false;
    pos = null;
    btnEl?.focus();
  }

  function choose(v: string) {
    onChange(v);
    close();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      // Both, and the pair matters: stopPropagation keeps the press from reaching
      // the enclosing modal's window listener, which would otherwise close the
      // modal along with the list. preventDefault marks it for anything listening
      // in the capture phase, where stopPropagation no longer helps.
      e.stopPropagation();
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      active = Math.min(options.length - 1, Math.max(0, active + step));
      // Keep the moving selection inside the scrolled box.
      listEl?.querySelectorAll(".opt")[active]?.scrollIntoView({ block: "nearest" });
      return;
    }
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (active >= 0) choose(options[active].value);
    }
  }

  function onBtnKey(e: KeyboardEvent) {
    // While the list is open the keys belong to it. Focus does not reliably move
    // to the list — it starts hidden for a frame, and a hidden element cannot take
    // focus — so the trigger stays focused and forwards them. This forwarding is
    // what makes Escape work: with the handler only on the list, the press landed
    // on the focused button, ran nothing, and reached the modal's window listener,
    // which closed the whole modal along with the list.
    if (open) {
      onKey(e);
      return;
    }
    if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      void show();
    }
  }
</script>

<!-- Reposition rather than close on resize: a modal's own scrolling moves the
     control under a list that would otherwise stay behind. -->
<svelte:window onresize={() => { if (open) void place(); }} />

<button
  bind:this={btnEl}
  type="button"
  class="sel-btn"
  class:placeholder={!selected}
  {disabled}
  {title}
  aria-label={ariaLabel}
  aria-haspopup="listbox"
  aria-expanded={open}
  onclick={() => (open ? close() : show())}
  onkeydown={onBtnKey}
>
  <span class="sel-label">{label}</span>
  <span class="sel-arrow" aria-hidden="true">▾</span>
</button>

{#if open}
  <!-- Closes on any press outside, and swallows that press so it does not also
       land on whatever is behind the list. -->
  <div
    class="sel-backdrop"
    onpointerdown={close}
    oncontextmenu={(e) => { e.preventDefault(); close(); }}
    role="presentation"
  ></div>

  <div
    bind:this={listEl}
    class="sel-list"
    class:measured={pos !== null}
    style="left:{pos?.x ?? 0}px; top:{pos?.y ?? 0}px; min-width:{width}px; max-height:{pos?.maxH ?? 0}px; max-width:{maxW}px;"
    role="listbox"
    tabindex="-1"
    onkeydown={onKey}
  >
    {#each options as o, i (o.value)}
      <button
        type="button"
        class="opt"
        class:sel={o.value === value}
        class:active={i === active}
        role="option"
        aria-selected={o.value === value}
        onclick={() => choose(o.value)}
        onmouseenter={() => (active = i)}
        title={o.label}
      >{o.label}</button>
    {/each}
  </div>
{/if}

<style>
  /* Matches input/select in app.css so the closed control is indistinguishable
     from the native ones still in use elsewhere. */
  .sel-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 5px 10px;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }

  .sel-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .sel-btn.placeholder .sel-label { color: var(--text-secondary); }

  .sel-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sel-arrow {
    flex-shrink: 0;
    font-size: 10px;
    color: var(--text-secondary);
  }

  .sel-backdrop {
    position: fixed;
    inset: 0;
    z-index: 200;
  }

  .sel-list {
    position: fixed;
    z-index: 201;
    display: flex;
    flex-direction: column;
    padding: 4px;
    overflow-y: auto;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 10px 32px rgba(0, 0, 0, 0.3);
    /* Hidden for the frame between mount and measurement — visibility, not
       display, because a display:none element cannot be measured. */
    visibility: hidden;
  }

  .sel-list.measured { visibility: visible; }
  .sel-list:focus-visible { outline: none; }

  .opt {
    flex-shrink: 0;
    /* Without this the button is sized by its own text and pushes the flex column
       wider than the list's max-width, which only bounds the box, not the content
       inside it. */
    min-width: 0;
    max-width: 100%;
    text-align: left;
    border: none;
    /* Square: a radius would pull the fill away from the rules above and below and
       leave each line looking broken at its ends. The popup's outer corners are
       rounded by .sel-list, which is what reads as its shape. */
    border-radius: 0;
    background: transparent;
    padding: 5px 10px;
    font-size: 13px;
    font-family: inherit;
    color: var(--text-primary);
    cursor: pointer;
    /* One line, ellipsised. Deliberately NOT -webkit-line-clamp: that draws the
       option as a -webkit-box, and in WebKitGTK — the engine the app actually runs
       in, unlike the Chromium the tests use — the clamp did not take, so nothing
       bounded the option and a paragraph-long task title stretched the list across
       the whole screen. text-overflow works in both.

       The cost is real: titles that differ only near the end look alike here. The
       full text is in the title attribute, and the alternative on screen was a
       dropdown the size of the window. */
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* A hairline between options. With every option truncated to one line, a list of
     task titles is a stack of near-identical text, and nothing told the eye where
     one entry ended and the next began.

     A rule rather than a gap: a gap separates without giving the eye anything to
     follow along, and over ten rows it reads as loose spacing rather than as
     structure. It goes on the top edge and is skipped on the first option, so the
     list does not open with a stray line under its own padding. */
  .opt + .opt {
    border-top: 1px solid var(--border);
  }

  /* Hover and keyboard share one highlight: two different ones on screen at the
     same time would read as two cursors. */
  .opt.active {
    background: var(--bg-hover);
  }

  /* The highlighted row swallows the rules touching it — its own and the one
     belonging to the option below — so the fill reads as one unbroken block rather
     than a band with a line cutting across each end. Transparent rather than
     removed: the row keeps its 1px and nothing shifts by a pixel.

     `.opt + .opt.active` for the row's own rule, so it outranks the `.opt + .opt`
     that draws it rather than relying on source order. */
  .opt + .opt.active,
  .opt.active + .opt {
    border-top-color: transparent;
  }

  .opt.sel {
    color: var(--accent);
    font-weight: 600;
  }
</style>
