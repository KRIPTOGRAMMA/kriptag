import { describe, it, expect } from "vitest";
import { clampMenu, clampTipX, placeDropdown } from "./menuPos";

// A window big enough that a 160x200 menu fits anywhere in the middle.
const WIN_W = 1000;
const WIN_H = 800;
const MENU_W = 160;
const MENU_H = 200;

describe("clampMenu", () => {
  it("оставляет меню на месте в середине окна", () => {
    expect(clampMenu(400, 300, MENU_W, MENU_H, WIN_W, WIN_H)).toEqual({ x: 400, y: 300 });
  });

  it("у нижнего края открывает меню вверх", () => {
    // 700 + 200 = 900 > 800: the bottom items would be off-window.
    const pos = clampMenu(400, 700, MENU_W, MENU_H, WIN_W, WIN_H);
    expect(pos.y).toBe(500);
    expect(pos.y + MENU_H).toBeLessThanOrEqual(WIN_H);
  });

  it("у правого края открывает меню влево", () => {
    const pos = clampMenu(900, 300, MENU_W, MENU_H, WIN_W, WIN_H);
    expect(pos.x).toBe(740);
    expect(pos.x + MENU_W).toBeLessThanOrEqual(WIN_W);
  });

  it("в правом нижнем углу переворачивает по обеим осям", () => {
    const pos = clampMenu(980, 780, MENU_W, MENU_H, WIN_W, WIN_H);
    expect(pos.x + MENU_W).toBeLessThanOrEqual(WIN_W);
    expect(pos.y + MENU_H).toBeLessThanOrEqual(WIN_H);
  });

  it("не выпускает меню за левый и верхний край после переворота", () => {
    // A window barely larger than the menu: flipping from near the origin would
    // put the menu at a negative coordinate.
    const pos = clampMenu(10, 10, MENU_W, MENU_H, 200, 220);
    expect(pos.x).toBeGreaterThanOrEqual(0);
    expect(pos.y).toBeGreaterThanOrEqual(0);
  });

  it("меню выше окна прижимается к верху, а не обрезается с обоих концов", () => {
    const pos = clampMenu(100, 300, MENU_W, 600, WIN_W, 400);
    expect(pos.y).toBeGreaterThanOrEqual(0);
    expect(pos.y).toBeLessThan(100);
  });
});

describe("clampTipX", () => {
  it("в середине контейнера подсказка следует за ячейкой", () => {
    expect(clampTipX(300, 280, 1000)).toBe(300);
  });

  // The regression: at 640 the tooltip used to stop following the cell entirely.
  it("правее старого потолка 640 подсказка всё ещё следует за ячейкой", () => {
    expect(clampTipX(700, 280, 1150)).toBe(700);
  });

  it("у правого края прижимается, не вылезая за контейнер", () => {
    expect(clampTipX(1100, 280, 1150)).toBe(870);
    expect(clampTipX(1100, 280, 1150) + 280).toBeLessThanOrEqual(1150);
  });

  it("контейнер уже подсказки — прижимается к левому краю", () => {
    expect(clampTipX(200, 280, 250)).toBe(0);
  });
});

describe("placeDropdown", () => {
  // A control 180px wide, 28px tall, in the middle of an 800x600 window.
  const ctl = { left: 200, top: 300, bottom: 328, width: 180 };

  it("по умолчанию открывается под контролом", () => {
    const p = placeDropdown(ctl, 120, 800, 600);
    expect(p.y).toBe(332);
    expect(p.x).toBe(200);
  });

  // The point of the whole function: a popup that covers its own control hides
  // the value being changed. clampMenu flips onto the cursor and would do that.
  it("никогда не накрывает сам контрол", () => {
    const low = { left: 200, top: 540, bottom: 568, width: 180 };
    const p = placeDropdown(low, 300, 800, 600);
    expect(p.y + Math.min(300, p.maxH)).toBeLessThanOrEqual(low.top);
  });

  it("внизу окна разворачивается вверх", () => {
    const low = { left: 200, top: 540, bottom: 568, width: 180 };
    const p = placeDropdown(low, 300, 800, 600);
    expect(p.y).toBeLessThan(low.top);
    expect(p.y).toBeGreaterThanOrEqual(0);
  });

  it("длинный список не выходит за окно, а ограничивается высотой", () => {
    const p = placeDropdown(ctl, 5000, 800, 600);
    expect(p.y + p.maxH).toBeLessThanOrEqual(600);
    expect(p.maxH).toBeGreaterThan(0);
  });

  it("у правого края сдвигается внутрь окна", () => {
    const right = { left: 700, top: 300, bottom: 328, width: 180 };
    const p = placeDropdown(right, 120, 800, 600);
    expect(p.x + right.width).toBeLessThanOrEqual(800);
  });

  // A list of task titles is wider than the control that opened it. The clamp used
  // the control's width, so the surplus hung off the right of the window — and in
  // a webview there is nothing to scroll to reach it.
  it("список шире контрола зажимается по своей ширине, а не по контролу", () => {
    // 700 in an 800 window with the control at x=200: clamping by the control's
    // width gives x=200 and a right edge of 900, off-window.
    const p = placeDropdown(ctl, 120, 800, 600, 4, 700);
    expect(p.x + 700).toBeLessThanOrEqual(800);
  });

  it("список шире окна прижимается к левому краю, а не уходит в минус", () => {
    const p = placeDropdown(ctl, 120, 300, 600, 4, 400);
    expect(p.x).toBeGreaterThanOrEqual(0);
  });

  it("без явной ширины поведение прежнее — по ширине контрола", () => {
    expect(placeDropdown(ctl, 120, 800, 600).x).toBe(200);
  });

  it("места мало с обеих сторон — остаётся снизу, но не уезжает за край", () => {
    // A window barely taller than the control: neither side fits the list.
    const p = placeDropdown({ left: 10, top: 30, bottom: 58, width: 180 }, 400, 800, 100);
    expect(p.maxH).toBeGreaterThanOrEqual(0);
    expect(p.y).toBeGreaterThanOrEqual(0);
  });
});
