// Rendering a number of minutes for a human. The dashboard used to print raw
// minutes everywhere, so a working day read as "384 мин" — a number nobody
// converts in their head while scanning a chart.
//
// A pure module rather than a helper inside Dashboard.svelte because vitest does
// not reach .svelte files, and the rounding here is worth pinning down.
//
// The translator is a parameter rather than an import of i18n.svelte: that module
// holds its language in $state, and a rune cannot be compiled by a plain .ts test
// (no other tested module under src/lib imports it, for the same reason).

// Mirrors the signature of t() in i18n.svelte.ts. A looser Record<string, unknown>
// here does not accept the real t(): the vars parameter is contravariant, so a
// wider one on this side is the incompatible direction.
export type Translate = (key: string, vars?: Record<string, string | number>) => string;

const MINS_PER_HOUR = 60;

// Minutes -> "45 мин" | "2 ч" | "6 ч 24 мин".
//
// The minutes part is dropped when it is zero so a round result reads "2 ч"
// rather than "2 ч 0 мин". Below an hour the hours part is absent entirely,
// which keeps short spans exact — rounding 45 minutes to "1 ч" would overstate
// the only case where the precise figure still matters.
//
// Negative input is not expected (durations are sums of non-negative spans) and
// is clamped to zero rather than rendered as "-1 ч -30 мин".
export function formatMinutes(mins: number, t: Translate): string {
  const total = Math.max(0, Math.round(mins));
  const h = Math.floor(total / MINS_PER_HOUR);
  const m = total % MINS_PER_HOUR;

  if (h === 0) return t("{n} мин", { n: m });
  if (m === 0) return t("{n} ч", { n: h });
  return t("{h} ч {m} мин", { h, m });
}
