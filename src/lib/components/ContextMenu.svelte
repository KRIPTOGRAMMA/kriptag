<script lang="ts">
  // A context menu at the cursor (v0.9.98).
  //
  // Written for the task row, whose six actions used to live in .task-actions and
  // appear on hover. Hover-only had two costs: the buttons were undiscoverable
  // (you learned about them by accident) and unreachable from the keyboard, and on
  // a long list six icons flickered in and out as the pointer crossed rows.
  //
  // The menu is deliberately dumb — it renders items and reports clicks. Deciding
  // what an item does, and whether it is currently on, belongs to the caller.
  import { onMount, tick } from "svelte";
  import { clampMenu } from "../menuPos";

  export interface MenuItem {
    label: string;
    onSelect: () => void;
    // Draws the item in --danger, for the one destructive entry.
    danger?: boolean;
    // Puts a separating line above this item.
    separated?: boolean;
    disabled?: boolean;
  }

  type Props = {
    x: number;
    y: number;
    items: MenuItem[];
    onClose: () => void;
  };

  let { x, y, items, onClose }: Props = $props();

  let el: HTMLElement | null = $state(null);
  // Off-screen until measured: the menu's own size is needed to know whether it
  // has to flip, and that is only knowable once it is in the DOM. Rendering at
  // the raw cursor first would show a frame in the wrong place near an edge.
  let pos = $state<{ x: number; y: number } | null>(null);

  onMount(() => {
    if (!el) return;
    const r = el.getBoundingClientRect();
    pos = clampMenu(x, y, r.width, r.height, window.innerWidth, window.innerHeight);
    // Focus the menu itself rather than the first item: the first item is not the
    // likeliest choice, and pre-selecting it invites an accidental Enter. This
    // runs after `pos` is set, because a visibility:hidden element cannot take
    // focus — focusing before the measurement silently does nothing.
    void tick().then(() => el?.focus());
  });

  // Escape is bound to the window rather than to the menu element. The element
  // starts hidden for one frame and can therefore lose the race for focus; a
  // menu that will not close on Escape is worse than one that closes too eagerly.
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  }

  function choose(item: MenuItem) {
    if (item.disabled) return;
    item.onSelect();
    onClose();
  }
</script>

<svelte:window on:resize={onClose} on:keydown={onKey} />

<!-- The backdrop closes the menu and swallows the click that would otherwise
     land on whatever is underneath. It is transparent, not dimmed: a context
     menu is a light interaction and darkening the screen overstates it. -->
<div
  class="ctxmenu-backdrop"
  onpointerdown={onClose}
  oncontextmenu={(e) => { e.preventDefault(); onClose(); }}
  role="presentation"
></div>

<div
  bind:this={el}
  class="ctxmenu"
  class:measured={pos !== null}
  style="left: {(pos?.x ?? x)}px; top: {(pos?.y ?? y)}px;"
  role="menu"
  tabindex="-1"
>
  {#each items as item (item.label)}
    <button
      type="button"
      role="menuitem"
      class="ctxmenu-item"
      class:danger={item.danger}
      class:separated={item.separated}
      disabled={item.disabled}
      onclick={() => choose(item)}
    >{item.label}</button>
  {/each}
</div>

<style>
  .ctxmenu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 200;
  }

  .ctxmenu {
    position: fixed;
    z-index: 201;
    min-width: 180px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 10px 32px rgba(0, 0, 0, 0.3);
    /* Hidden for the one frame between mount and measurement. visibility rather
       than display: the element must still have a size to be measured. */
    visibility: hidden;
  }

  .ctxmenu.measured {
    visibility: visible;
  }

  .ctxmenu:focus-visible {
    outline: none;
  }

  .ctxmenu-item {
    text-align: left;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    padding: 5px 10px;
    font-size: 13px;
    color: var(--text-primary);
    white-space: nowrap;
  }

  .ctxmenu-item:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .ctxmenu-item.danger {
    color: var(--danger);
  }

  .ctxmenu-item.danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--danger) 10%, transparent);
  }

  .ctxmenu-item.separated {
    margin-top: 4px;
    border-top: 1px solid var(--border);
    border-radius: 0 0 var(--radius) var(--radius);
    padding-top: 8px;
  }
</style>
