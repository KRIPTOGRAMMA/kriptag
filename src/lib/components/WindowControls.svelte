<script lang="ts">
  // Our own window buttons in place of the system title bar.
  //
  // The main window is declared with decorations: false, so WebKitGTK no longer draws
  // the white bar with the title — and with it go the system buttons and the ability
  // to drag the window with the mouse. Both are restored here.

  //
  // The drag region is not data-tauri-drag-region but an explicit startDragging call
  // on mousedown: the attribute fires on any click inside the marked element,
  // including clicks on nested buttons, and then pressing "minimize" would turn into
  // a micro-drag instead of a click.

  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Icon from "./Icon.svelte";
  import { t } from "../i18n.svelte";

  let maximized = $state(false);

  // The "maximize" button must show the current state: maximizing is also possible by
  // double-clicking the header and through the WM, so the icon has to follow the
  // window rather than its own clicks.
  $effect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    let alive = true;
    win.isMaximized().then((v) => { if (alive) maximized = v; }).catch(() => {});
    win.onResized(() => {
      win.isMaximized().then((v) => { if (alive) maximized = v; }).catch(() => {});
    }).then((fn) => {
      if (alive) unlisten = fn; else fn();
    }).catch(() => {});
    return () => { alive = false; unlisten?.(); };
  });

  async function startDrag(e: MouseEvent) {
    // The primary button only: a right click on the header belongs to the WM.
    if (e.button !== 0) return;
    try {
      await getCurrentWindow().startDragging();
    } catch {
      // Dragging is not an operation worth breaking the UI over.
    }
  }

  async function toggleMaximize() {
    try {
      await getCurrentWindow().toggleMaximize();
    } catch {
      /* see above */
    }
  }

  async function minimize() {
    try {
      await getCurrentWindow().minimize();
    } catch {
      /* see above */
    }
  }

  // Closing hides the window rather than ending the process: the app lives in the
  // tray and the background loops (tracking, pomodoro, notifications) must keep
  // running. Quitting is only available from the tray menu, as it was with the
  // system button.
  async function close() {
    try {
      await getCurrentWindow().hide();
    } catch {
      /* see above */
    }
  }
</script>

<!-- The drag strip is its own element rather than the bar itself. The bar has to
     keep pointer-events: none so clicks reach the content beneath it, and an
     element that does not receive the pointer does not receive mousedown either:
     with the handler on the bar, the only draggable spot was the button group,
     which is where its pointer-events are re-enabled. On Linux the window
     manager offers Super+drag as a fallback, so it went unnoticed; Windows has
     no such gesture and the window could not be moved at all. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="titlebar">
  <div class="drag-strip" onmousedown={startDrag} ondblclick={toggleMaximize}></div>
  <div class="controls">
    <button class="win-btn" onclick={minimize} title={t("Свернуть")} aria-label={t("Свернуть")}>
      <Icon name="winmin" size={14} />
    </button>
    <button
      class="win-btn"
      onclick={toggleMaximize}
      title={maximized ? t("Восстановить") : t("Развернуть")}
      aria-label={maximized ? t("Восстановить") : t("Развернуть")}
    >
      <Icon name={maximized ? "winrestore" : "winmax"} size={14} />
    </button>
    <button class="win-btn close" onclick={close} title={t("Закрыть")} aria-label={t("Закрыть")}>
      <Icon name="winclose" size={14} />
    </button>
  </div>
</div>

<style>
  /* Floats above the content and takes no height of its own, so the views do not
     have to be pushed down by a header's height. It spans the full width, so the
     whole top edge of the window acts as a drag region. */
  .titlebar {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    height: 32px;
    display: flex;
    justify-content: flex-end;
    align-items: center;
    padding-right: 6px;
    z-index: 60;
    /* Clicks pass through everywhere except the buttons themselves and the region
       on the right: otherwise the header would intercept the top line of the
       content. */
    pointer-events: none;
  }

  /* Fills the bar and takes the pointer back, so the whole top edge drags the
     window — everywhere except the buttons, which come after it in the flex row
     and keep their own hit area.

     It stops short of the sidebar on the left: .brand sits at the very top there
     and the navigation right below it, and a strip over that column would eat
     the first click on it. 176px is the sidebar's width (App.svelte). The height
     is the bar's own 32px, and .content is already padded by exactly that much,
     so the strip covers reserved space only and no view loses its first row. */
  .drag-strip {
    position: absolute;
    top: 0;
    left: 176px;
    right: 0;
    height: 32px;
    pointer-events: auto;
  }

  .controls {
    display: flex;
    gap: 2px;
    pointer-events: auto;
    /* Above the strip, which spans the full width beneath them. */
    position: relative;
    z-index: 1;
  }

  .win-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-secondary);
  }

  .win-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .win-btn.close:hover {
    background: var(--danger);
    color: #fff;
  }
</style>
