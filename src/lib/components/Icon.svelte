<script lang="ts">
  // Inline SVG icons instead of emoji: emoji glyphs (🍅 ▶ ⏸ 📅 …) render
  // unpredictably on Linux — they depend on which emoji fonts are installed.
  // The same pattern as the sidebar navigation in App.svelte:
  // stroke="currentColor", with no external fonts or CDNs.
  const PATHS: Record<string, string> = {
    play:     "M7 4.5v15l13-7.5Z",
    stop:     "M6.5 6.5h11v11h-11Z",
    pause:    "M9 5v14 M15 5v14",
    skip:     "M5 5v14l9-7Z M18 5v14",
    calendar: "M5 5h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2Z M16 3v4 M8 3v4 M3 11h18",
    link:     "M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71 M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71",
    sparkles: "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9Z M19 15l.9 2.1L22 18l-2.1.9L19 21l-.9-2.1L16 18l2.1-.9Z",
    clock:    "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z M12 7v5l3 2",
    zap:      "M13 2 3 14h9l-1 8 10-12h-9l1-8Z",
    shuffle:  "M16 3h5v5 M4 20 21 3 M21 16v5h-5 M15 15l6 6 M4 4l5 5",
    pencil:   "M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z",
    coffee:   "M17 8h1a4 4 0 1 1 0 8h-1 M3 8h14v9a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4Z",
    timer:    "M10 2h4 M12 14l3-3 M12 22a8 8 0 1 0 0-16 8 8 0 0 0 0 16Z",
    tag:      "M20.6 13.4 12 22 2 12V2h10l8.6 8.6a2 2 0 0 1 0 2.8Z M7 7h.01",
    flag:     "M4 21V3 M4 4h12l-2 4 2 4H4",
    target:   "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Z M12 18a6 6 0 1 0 0-12 6 6 0 0 0 0 12Z M12 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4Z",
    lock:     "M5 11h14a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1Z M8 11V7a4 4 0 0 1 8 0v4",
    alert:    "M12 3 2 20h20Z M12 9v5 M12 17h.01",
    pin:      "M9 4h6 M10 4v5.5L6 14v2h5v6l1 2 1-2v-6h5v-2l-4-4.5V4",
    expand:   "M8 3H5a2 2 0 0 0-2 2v3 M16 3h3a2 2 0 0 1 2 2v3 M21 16v3a2 2 0 0 1-2 2h-3 M3 16v3a2 2 0 0 0 2 2h3",
    collapse: "M9 3v3a2 2 0 0 1-2 2H4 M15 3v3a2 2 0 0 0 2 2h3 M15 21v-3a2 2 0 0 1 2-2h3 M9 21v-3a2 2 0 0 0-2-2H4",
    bold:     "M7 4h6.5a3.5 3.5 0 0 1 0 7H7Z M7 11h7a3.5 3.5 0 0 1 0 7H7Z",
    italic:   "M11 4h6 M7 20h6 M14 4 10 20",
    heading:  "M6 4v16 M18 4v16 M6 12h12",
    checklist:"M9 6h11 M9 12h11 M9 18h11 M4 6l1.5 1.5L7.5 5 M4 12l1.5 1.5L7.5 11 M4 18l1.5 1.5L7.5 16",
    wikilink: "M7 8H6a4 4 0 0 0 0 8h1 M17 8h1a4 4 0 0 1 0 8h-1 M9 12h6",
    code:     "M9 6 3 12l6 6 M15 6l6 6-6 6",
    // A quote is a vertical bar plus lines of text; an ordered list is
    // "1." / "2." as strokes on the left with lines on the right.
    quote:    "M4 5v14 M9 8h11 M9 12h11 M9 16h7",
    orderlist:"M4 6h1v4 M4 10h2 M4 14h2v2H4v2h2 M10 7h10 M10 12h10 M10 17h10",
    table:    "M3 4h18v16H3Z M3 10h18 M3 16h18 M9 4v16 M15 4v16",
    export:   "M12 15V3 M7 8l5-5 5 5 M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4",
    sun:      "M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10Z M12 1v2 M12 21v2 M4.22 4.22l1.42 1.42 M18.36 18.36l1.42 1.42 M1 12h2 M21 12h2 M4.22 19.78l1.42-1.42 M18.36 5.64l1.42-1.42",
    bell:     "M6 8a6 6 0 0 1 12 0c0 4 1.5 6 2 7H4c.5-1 2-3 2-7Z M10 20a2 2 0 0 0 4 0",
    columns:  "M3 4h18v16H3Z M9 4v16 M15 4v16",
    // Window buttons in place of the system title bar. Drawn with the same
    // stroke="currentColor" as the rest — thin GNOME-style strokes rather than
    // heavy glyphs.
    winmin:   "M5 12h14",
    winmax:   "M5 5h14v14H5Z",
    // "Restore down": the standard two offset rectangles. The maximise button used
    // to borrow `collapse` (four inward corner brackets) for this state — that icon
    // belongs to zen mode and read as four detached corners rather than a window.
    winrestore: "M8 8h11v11H8Z M5 16V5h11",
    winclose: "M6 6l12 12 M18 6 6 18",
  };

  let { name, size = 14 }: { name: string; size?: number } = $props();
</script>

<svg viewBox="0 0 24 24" width={size} height={size} fill="none"
  stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"
  style="vertical-align:-0.125em;flex-shrink:0;">
  <path d={PATHS[name] ?? ""} />
</svg>
