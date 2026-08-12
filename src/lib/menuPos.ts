// Where a context menu opens (v0.9.98).
//
// A menu is placed at the cursor, which near a window edge would put part of it
// outside the window — and in a desktop webview there is nothing to scroll to
// reach it, so those items are simply unreachable. Flipping the menu back over
// the cursor is the standard answer.
//
// This lives in its own .ts rather than inside ContextMenu.svelte because vitest
// does not test .svelte files: the decision belongs here, the component only
// applies the result. Same split as uistate.ts and surfaces.ts.

export interface MenuPos {
  x: number;
  y: number;
}

/** Keeps the window edge this far from the menu, so it never sits flush. */
const EDGE_GAP = 4;

/**
 * Clamps a menu of the given size into the window.
 *
 * Horizontally the menu flips to the left of the cursor, vertically to above it.
 * Flipping is preferred over shifting because a shifted menu covers the very row
 * that was right-clicked, and the user loses sight of what they are acting on.
 *
 * When the menu is larger than the window in an axis, it is pinned to the top or
 * left edge: showing its start beats centring it and cutting off both ends.
 */
export function clampMenu(
  x: number,
  y: number,
  menuW: number,
  menuH: number,
  winW: number,
  winH: number,
): MenuPos {
  let outX = x;
  let outY = y;

  if (x + menuW + EDGE_GAP > winW) outX = x - menuW;
  if (y + menuH + EDGE_GAP > winH) outY = y - menuH;

  // The flip itself can push the menu off the opposite edge — on a small window,
  // or with the cursor near the top-left corner.
  if (outX < EDGE_GAP) outX = EDGE_GAP;
  if (outY < EDGE_GAP) outY = EDGE_GAP;

  return { x: outX, y: outY };
}

// Clamps a tooltip's left edge so it stays inside the box it is positioned
// against — for the calendar tooltip that is the viewport, since the element is
// position:fixed at the top level.
//
// Separate from clampMenu: that one flips a menu to the other side of the cursor,
// which is right for a menu (it must never cover the thing it was opened from) and
// wrong for a tooltip anchored under a 16px cell — flipping would put it above the
// row the user is reading along.
//
// The calendar tooltip used a hardcoded ceiling of 640px: past that point it
// stopped tracking the cell and parked mid-panel.
export function clampTipX(x: number, tipW: number, containerW: number): number {
  // Nothing sensible to do when the tooltip is wider than the box; pinning to the
  // left edge at least keeps the beginning of the text readable.
  const maxX = Math.max(0, containerW - tipW);
  return Math.min(Math.max(0, x), maxX);
}

export interface DropPos {
  x: number;
  y: number;
  /** How tall the list may be here; it scrolls internally beyond this. */
  maxH: number;
}

/**
 * Places a dropdown list against the control that opened it.
 *
 * Unlike clampMenu this anchors to a box rather than to a point, and it never
 * flips onto the trigger: a select's popup that covered its own control would
 * hide the current value while you pick a new one. It opens below by default and
 * above only when below is the smaller side, taking the taller of the two.
 *
 * The height is returned rather than assumed because the caller cannot know it —
 * the list is as long as its options, and the room below a control near the
 * bottom of a modal is whatever is left.
 */
export function placeDropdown(
  rect: { left: number; bottom: number; top: number; width: number },
  listH: number,
  winW: number,
  winH: number,
  gap = 4,
  // The list's own width, which is at least the trigger's but grows with the
  // longest option. Clamping by the trigger's width was wrong: a dropdown of task
  // titles is far wider than the control that opens it, and the surplus hung off
  // the right of the window where there is nothing to scroll to reach it.
  listW = rect.width,
): DropPos {
  const below = winH - rect.bottom - gap - EDGE_GAP;
  const above = rect.top - gap - EDGE_GAP;
  // Prefer below — reading downwards matches where the list visually unrolls —
  // and only give that up when the list does not fit and there is more room up.
  const openUp = listH > below && above > below;
  const maxH = Math.max(0, openUp ? above : below);
  const h = Math.min(listH, maxH);

  const x = Math.min(Math.max(EDGE_GAP, rect.left), Math.max(EDGE_GAP, winW - listW - EDGE_GAP));
  const y = openUp ? rect.top - gap - h : rect.bottom + gap;

  return { x, y, maxH };
}
