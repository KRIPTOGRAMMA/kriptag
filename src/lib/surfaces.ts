// Deriving the whole surface stack from one background colour.
//
// Until now four backgrounds were four independent settings: --bg-primary,
// --bg-secondary, --bg-hover and --bg-card. Only three of them were reachable
// from the UI, so painting the background red left the task rows sitting on the
// old dark --bg-card. Opening the fourth would not have fixed it: four free
// numbers cannot express "these surfaces sit above each other", which is the
// only thing they are ever meant to say.
//
// So one colour is chosen and the rest are computed from it. The relationships
// are then true by construction and cannot be broken by a careless pick.
//
// Direction depends on the base: on a light background surfaces step DOWN
// towards darker, on a dark one UP towards lighter. That is why a plain
// lighten() does not work here — on white it would have nowhere to go.

export interface Surfaces {
  bgSecondary: string;
  bgCard: string;
  bgHover: string;
  border: string;
  textPrimary: string;
  textSecondary: string;
}

interface Rgb { r: number; g: number; b: number }

export function parseHex(hex: string): Rgb | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  let h = m[1];
  if (h.length === 3) h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
  const n = parseInt(h, 16);
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff };
}

function toHex({ r, g, b }: Rgb): string {
  const c = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
  return `#${((c(r) << 16) | (c(g) << 8) | c(b)).toString(16).padStart(6, "0")}`;
}

// Relative luminance, sRGB (WCAG 2.1). Used both to decide the direction of the
// steps and to pick readable text.
export function luminance({ r, g, b }: Rgb): number {
  const ch = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b);
}

export function isDarkColor(hex: string): boolean {
  const rgb = parseHex(hex);
  return rgb === null ? false : luminance(rgb) < 0.5;
}

// One step away from the base, keeping its hue: mixing towards white or black
// rather than scaling channels, so a saturated red stays red instead of drifting
// towards pink as it lightens.
function step(base: Rgb, amount: number, towardsLight: boolean): Rgb {
  const target = towardsLight ? 255 : 0;
  return {
    r: base.r + (target - base.r) * amount,
    g: base.g + (target - base.g) * amount,
    b: base.b + (target - base.b) * amount,
  };
}

// The step sizes. Deliberately small: these surfaces have to read as one family,
// and the difference between a panel and a card is a hint, not a statement. The
// border gets the largest step because it is a 1px line — the same shift that
// reads as plenty on a filled area is barely visible on a hairline.
const STEPS = {
  bgSecondary: 0.06,
  bgCard: 0.09,
  bgHover: 0.14,
  border: 0.2,
};

// The card is the exception, and the stock light theme is why: there
// --bg-secondary is #f4f2f8 while --bg-card is pure #ffffff. A card is a raised
// surface — it catches the light — so on a light background it steps towards
// white while everything else steps away from it. Sending it down with the rest
// turned white backgrounds visibly grey, which is not what the theme it is
// imitating does.
const CARD_STEP_ON_LIGHT = 0.55;

// The text that sits ON the accent — filled buttons, the active segment. White
// is only the right answer for a dark accent: the pastel accents of the popular
// dark themes (Nord #88c0d0, Gruvbox #fabd2f) give white 1.7–2.4, well below
// legibility, while near-black on them is comfortable.
export function onAccentText(hex: string): string {
  const rgb = parseHex(hex);
  if (rgb === null) return "#ffffff";
  // Picked by measured contrast rather than a luminance cutoff: a threshold
  // would have to be tuned by hand and still mislabels the middle of the range,
  // where Indigo (white wins 4.47 to 4.12) and Ember (dark wins 6.50 to 2.84)
  // sit close together.
  const l = luminance(rgb);
  const withWhite = 1.05 / (l + 0.05);
  const withDark = (l + 0.05) / (luminance({ r: 20, g: 20, b: 20 }) + 0.05);
  return withWhite >= withDark ? "#ffffff" : "#141414";
}

// White or near-black text, whichever contrasts better with the surface.
function readableText(base: Rgb): { primary: string; secondary: string } {
  const light = luminance(base) < 0.5;
  return light
    ? { primary: "#f5f5f5", secondary: "#a0a0a0" }
    : { primary: "#1a1a1a", secondary: "#666666" };
}

// Derives every surface from the background. Returns null for anything that is
// not a hex colour, so the caller falls back to the stylesheet defaults.
export function deriveSurfaces(bg: string): Surfaces | null {
  const base = parseHex(bg);
  if (base === null) return null;

  // On a dark background the stack climbs towards light, on a light one it
  // descends towards dark. Both directions stay inside the same hue.
  const up = luminance(base) < 0.5;
  const text = readableText(base);

  return {
    bgSecondary: toHex(step(base, STEPS.bgSecondary, up)),
    // On a dark background the card rides the same ladder as everything else; on
    // a light one it goes the other way, towards white. See CARD_STEP_ON_LIGHT.
    bgCard: toHex(up ? step(base, STEPS.bgCard, true) : step(base, CARD_STEP_ON_LIGHT, true)),
    bgHover: toHex(step(base, STEPS.bgHover, up)),
    border: toHex(step(base, STEPS.border, up)),
    textPrimary: text.primary,
    textSecondary: text.secondary,
  };
}
