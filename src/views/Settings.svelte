<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  // confirm comes from the plugin, not from the webview. The browser's confirm()
  // is a blocking native dialog, and on Linux/WebKitGTK opening one right after
  // the file chooser kills the process — reported from a real install: the app
  // closed the moment a file was picked. The plugin's version goes through the
  // same async dialog queue as open(), so the two can follow one another.
  import { save as saveDialog, open as openDialog, confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
  import { api } from "../lib/api/tauri";
  import { categoryStore } from "../lib/stores/categories.svelte";
  import { statusStore } from "../lib/stores/statuses.svelte";
  import type { AppSettings, AppCategoryRule, AppLimit, GlobalAction } from "../lib/types";
  import { applyTheme } from "../lib/theme";
  import Switch from "../lib/components/Switch.svelte";
  import Select from "../lib/components/Select.svelte";
  import { colorSwatch, isDefaultColor, type ColorKey } from "../lib/colorDefaults";
  import { isDarkColor } from "../lib/surfaces";
  import {
    parsePresets, serializePresets, presetFromColors, addPreset, removePreset,
    PRESET_COLOR_KEYS, MAX_PRESETS, MAX_NAME_LEN, type ThemePreset,
  } from "../lib/themePresets";
  import ModelDownloader from "../lib/components/ModelDownloader.svelte";
  import Icon from "../lib/components/Icon.svelte";
  import { HELP_TOPICS } from "../lib/help";
  import { backupHealth } from "../lib/backupHealth";
  import { localeTag } from "../lib/datetime";
  import { LANGS, SEEDED_CATEGORY_IDS, type Lang } from "../lib/i18n";
  import { i18n, t, tErr } from "../lib/i18n.svelte";
  import {
    KEYBIND_ACTIONS, type Keybinds,
    parseKeybinds, comboFor, comboFromEvent, formatCombo, findConflicts,
  } from "../lib/keybinds";
  import { loadUiState, saveUiState, restoreOneOf } from "../lib/uistate";

  const PROVIDERS: { value: AppSettings["ai_provider"]; label: string }[] = $derived([
    { value: "none", label: t("Без ИИ (функции отключены)") },
    { value: "local", label: t("Локальная модель (llamafile)") },
    { value: "openai", label: "OpenAI" },
    { value: "anthropic", label: "Anthropic" },
  ]);

  // Each preset sets a pair of accents (primary plus secondary, the .btn-primary
  // gradient) with one button; "Custom" leaves the manual pickers below untouched.
  // `bg` is optional: most presets only recolour the accents and leave whatever
  // background is in effect, but one that carries its own ground has to set it,
  // since every other surface is derived from that one value.
  const THEME_PRESETS: { name: string; accent: string; accentSecondary: string; bg?: string }[] = $derived([
    // Matches the app.css defaults, so this preset is the way back to the
    // stock look after trying the others.
    { name: "Indigo", accent: "#6366f1", accentSecondary: "#a855f7" },
    // The rest carry their own ground. Grounds and accents are taken from the
    // established editor palettes, but every pair was re-measured against the
    // surfaces this app derives from them: the accent clears 4.5 both on the
    // background and on a card, which is what the originals do not guarantee —
    // they were tuned for syntax on one flat colour, not for UI on a stack.
    { name: "Ember", accent: "#ff6b35", accentSecondary: "#ff9f1c", bg: "#0d1117" },
    { name: "Nord", accent: "#88c0d0", accentSecondary: "#b48ead", bg: "#2e3440" },
    { name: "Dracula", accent: "#c9a9fb", accentSecondary: "#ff79c6", bg: "#282a36" },
    { name: "Tokyo", accent: "#7aa2f7", accentSecondary: "#bb9af7", bg: "#1a1b26" },
    { name: "Gruvbox", accent: "#fabd2f", accentSecondary: "#fe8019", bg: "#282828" },
    { name: "Everforest", accent: "#a7c080", accentSecondary: "#dbbc7f", bg: "#2d353b" },
    // The light half. Their accents are deliberately darker than the originals:
    // Solarized's own #268bd2 gives 3.56 on a card, below legibility, because a
    // card here sits lighter than the ground it came from.
    { name: "Solarized", accent: "#1a6ea8", accentSecondary: "#1a7f74", bg: "#fdf6e3" },
    { name: t("Роза"), accent: "#6f5b87", accentSecondary: "#a8524f", bg: "#faf4ed" },
    { name: t("Песок"), accent: "#8a5a2b", accentSecondary: "#a0522d", bg: "#f5f0e8" },
    // The user's own trials. Slate's ground is darker than the #4A5568 that was
    // sent: a mid-luminance ground sank both accents (2.52 and 1.49), because
    // there is no room to derive a stack from it in either direction. Its accent
    // was lifted for the same reason — #E8744F read 3.76 on a card.
    { name: t("Латунь"), accent: "#c8a96e", accentSecondary: "#e0c892", bg: "#111111" },
    { name: t("Полярный"), accent: "#e4f0f6", accentSecondary: "#8fb8d8", bg: "#0a0f1e" },
    { name: t("Неон"), accent: "#ff3d8b", accentSecondary: "#ff6b9d", bg: "#080808" },
    { name: t("Пергамент"), accent: "#f0e6c8", accentSecondary: "#c8b88a", bg: "#1c1c1e" },
    { name: t("Серебро"), accent: "#c0c0c0", accentSecondary: "#c41e3a", bg: "#0a0a0a" },
    { name: t("Сланец"), accent: "#f59b76", accentSecondary: "#3fa9a6", bg: "#232833" },
    { name: t("Орхидея"), accent: "#e8b4d8", accentSecondary: "#b48ead", bg: "#12111a" },
  ]);

  // Split by how much damage a wrong value does. The accent and the backgrounds
  // are what people come here to change; text and borders are one bad pick away
  // from an unreadable screen, so they live behind a disclosure.
  const MAIN_COLORS: [ColorKey, string][] = $derived([
    ["color_accent", t("Акцент")],
    ["color_accent_secondary", t("Доп. акцент")],
    ["color_bg", t("Фон")],
  ]);

  // Everything here is derived from the background by default (see surfaces.ts);
  // these fields override one step of that stack. "Second plane" rather than
  // "Sidebar background": the token paints the sidebar plus two dozen other
  // second-plane surfaces — the calendar backlog, the graph, dashboard panels —
  // and the old label promised something narrower than it delivered.
  // Grouped by layer rather than listed flat: seven pickers in one grid read as
  // a wall, and the grouping is also the explanation of what derives from what.
  const ADVANCED_GROUPS: { title: string; colors: [ColorKey, string][] }[] = $derived([
    {
      title: t("Поверхности"),
      colors: [
        ["color_bg_secondary", t("Второй план")],
        ["color_bg_card", t("Карточки")],
        ["color_bg_hover", t("Фон наведения")],
        ["color_border", t("Границы")],
      ],
    },
    {
      title: t("Текст"),
      colors: [
        ["color_text", t("Основной")],
        ["color_text_secondary", t("Подписи")],
      ],
    },
  ]);

  // Which theme is on screen right now — the swatch of an unset colour has to
  // show that theme's default, not the light one. `theme_mode` alone is not
  // enough: under "system" the answer comes from the OS.
  let isDark = $state(false);
  function syncIsDark() {
    if (typeof document !== "undefined") {
      isDark = document.documentElement.classList.contains("dark");
    }
  }

  // The theme is applied on every change — a live preview without pressing "Save".
  function previewTheme() {
    applyTheme(settings.theme_mode, settings);
    // applyTheme is what toggles the `dark` class, so the swatches of unset
    // colours only follow a theme switch if they are re-read afterwards.
    syncIsDark();
  }

  // Switching light/dark/system means "give me this mode's own palette". The
  // derived surfaces are written inline on <html>, and inline beats any class —
  // so a background left over from a dark preset would survive the switch and
  // the light theme would appear broken, with no way out from the UI. The accents
  // stay: those are a choice about the app, not about the mode.
  function switchThemeMode() {
    resetBackground();
  }

  // Drops the chosen ground and every override derived from it, leaving the
  // accents alone. Used both by the mode switch and by the button next to the
  // background picker.
  function resetBackground() {
    settings.color_bg = "";
    settings.color_bg_secondary = "";
    settings.color_bg_hover = "";
    settings.color_bg_card = "";
    settings.color_text_secondary = "";
    settings.color_text = "";
    settings.color_border = "";
    previewTheme();
  }

  function applyPreset(accent: string, accentSecondary: string, bg?: string) {
    settings.color_accent = accent;
    settings.color_accent_secondary = accentSecondary;
    // A preset without its own ground leaves the current one alone; one that has
    // it also clears the manual overrides of the derived surfaces, or a leftover
    // "second plane" from an earlier choice would sit on top of the new ground.
    if (bg) {
      settings.color_bg = bg;
      settings.color_bg_secondary = "";
      settings.color_bg_hover = "";
      settings.color_bg_card = "";
      settings.color_text_secondary = "";
      settings.color_text = "";
      settings.color_border = "";
      // The mode follows the ground. The derived surfaces cover only what is
      // computed from the background; the `dark` class still governs everything
      // else — category colours, the tag chip, --danger. Leaving a light preset
      // under `.dark` would paint those for the wrong ground.
      settings.theme_mode = isDarkColor(bg) ? "dark" : "light";
    }
    previewTheme();
  }

  function resetColors() {
    settings.color_accent = "";
    settings.color_accent_secondary = "";
    settings.color_bg = "";
    settings.color_bg_secondary = "";
    settings.color_bg_hover = "";
    settings.color_bg_card = "";
    settings.color_text_secondary = "";
    settings.color_text = "";
    settings.color_border = "";
    previewTheme();
  }

  // --- Saved colour sets ---
  //
  // Unlike the built-in presets, which carry a pair of accents, a saved set
  // restores all seven colours: it is a whole look, and reapplying the accents
  // over someone else's background would be worse than doing nothing.
  let customPresets: ThemePreset[] = $state([]);
  let presetName = $state("");

  function saveCurrentAsPreset() {
    const name = presetName.trim();
    if (!name) return;
    customPresets = addPreset(customPresets, presetFromColors(name, settings as any));
    settings.custom_theme_presets = serializePresets(customPresets);
    presetName = "";
  }

  // --- Preview on hover ---
  //
  // A swatch shows two accents; it says nothing about what the ground does to
  // cards, borders and captions — which is most of what a preset changes. So
  // hovering applies the theme for real and leaving puts back what was chosen.
  //
  // Nothing is written to `settings`: the preview only touches the CSS variables
  // on <html>, so leaving the row — or the screen — restores the saved state,
  // and a preset seen but not clicked never reaches the DB.
  function previewColors(colors: Partial<AppSettings>) {
    const mode = colors.color_bg
      ? (isDarkColor(colors.color_bg) ? "dark" : "light")
      : settings.theme_mode;
    applyTheme(mode as typeof settings.theme_mode, { ...settings, ...colors });
  }

  function endPreview() {
    previewTheme();
  }

  function previewPreset(p: { accent: string; accentSecondary: string; bg?: string }) {
    // The same clearing applyPreset does: without it a manual "second plane"
    // would sit on top of the previewed ground and the preview would lie.
    previewColors({
      color_accent: p.accent,
      color_accent_secondary: p.accentSecondary,
      ...(p.bg
        ? {
            color_bg: p.bg,
            color_bg_secondary: "",
            color_bg_hover: "",
            color_bg_card: "",
            color_text_secondary: "",
            color_text: "",
            color_border: "",
          }
        : {}),
    });
  }

  function previewCustomPreset(preset: ThemePreset) {
    const colors: Partial<AppSettings> = {};
    for (const key of PRESET_COLOR_KEYS) (colors as any)[key] = preset.colors[key] ?? "";
    previewColors(colors);
  }

  function applyCustomPreset(preset: ThemePreset) {
    for (const key of PRESET_COLOR_KEYS) {
      (settings as any)[key] = preset.colors[key] ?? "";
    }
    previewTheme();
  }

  function deleteCustomPreset(name: string) {
    customPresets = removePreset(customPresets, name);
    settings.custom_theme_presets = serializePresets(customPresets);
  }

  // A saved set is shown as the same two-stop gradient the built-in presets use,
  // falling back to the theme default when a colour follows it.
  function presetSwatch(preset: ThemePreset): string {
    const a = colorSwatch("color_accent", preset.colors.color_accent ?? "", isDark);
    const b = colorSwatch("color_accent_secondary", preset.colors.color_accent_secondary ?? "", isDark);
    return `linear-gradient(135deg, ${a}, ${b})`;
  }

  let settings: AppSettings = $state({
    ai_provider: "local",
    openai_key: "",
    openai_model: "gpt-4o-mini",
    anthropic_key: "",
    anthropic_model: "claude-haiku-4-5-20251001",
    idle_threshold_secs: 300,
    log_interval_secs: 60,
    work_mode: "Light",
    onboarding_complete: true,
    deadline_warn_hours: 24,
    deadline_warn_minutes: 60,
    idle_notify_min_mins: 10,
    pomodoro_work_mins: 25,
    pomodoro_break_mins: 5,
    nudge_after_mins: 90,
    theme_mode: "system",
    color_accent: "",
    color_accent_secondary: "",
    color_bg: "",
    color_bg_secondary: "",
    color_bg_hover: "",
    color_bg_card: "",
    color_text_secondary: "",
    color_text: "",
    color_border: "",
    quiet_until: "",
    context_notifications: true,
    ai_fallback: false,
    openai_in_keyring: false,
    anthropic_in_keyring: false,
    custom_theme_presets: "",
    app_category_rules: "",
    app_limits: "",
    auto_backup_dir: "",
    auto_backup_keep: 7,
    last_auto_backup: "",
    last_auto_backup_error: "",
    morning_digest_time: "",
    show_subtasks_expanded: true,
    keybinds: "",
    global_keybinds: "",
    focus_mode_auto: true,
    track_domains: false,
    language: "",
    history_cleanup_months: 0,
  });

  // --- The preset dropdown ---
  //
  // Seventeen presets no longer fit a row of buttons. A listbox rather than a
  // native <select>, because the gradient swatch is half of what a preset says
  // and <option> cannot carry one.
  let presetOpen = $state(false);
  let presetOptionEls: HTMLButtonElement[] = $state([]);
  // The saved sets get their own dropdown with the same behaviour; the handlers
  // below take the list they act on rather than being written twice.
  let customOpen = $state(false);
  let customOptionEls: HTMLButtonElement[] = $state([]);

  // Which preset the current colours correspond to. Compared by value rather
  // than remembered on click: the accents can also be changed by hand, by a
  // saved set, or by a reset, and a remembered name would keep claiming a preset
  // that is no longer in effect.
  const currentPreset = $derived(
    THEME_PRESETS.find(
      (p) =>
        p.accent === settings.color_accent &&
        p.accentSecondary === settings.color_accent_secondary &&
        (p.bg ?? "") === settings.color_bg,
    ) ?? null,
  );

  const currentPresetName = $derived(currentPreset?.name ?? t("Свои цвета"));

  const currentPresetSwatch = $derived(
    currentPreset
      ? `linear-gradient(135deg, ${currentPreset.accent}, ${currentPreset.accentSecondary})`
      : `linear-gradient(135deg, ${colorSwatch("color_accent", settings.color_accent, isDark)}, ${colorSwatch("color_accent_secondary", settings.color_accent_secondary, isDark)})`,
  );

  function choosePreset(p: { accent: string; accentSecondary: string; bg?: string }) {
    applyPreset(p.accent, p.accentSecondary, p.bg);
    presetOpen = false;
  }

  // Focus moving outside the whole control closes it; moving between the trigger
  // and the options does not. relatedTarget is null when focus leaves the window
  // entirely — the list stays open then, so switching to another window and back
  // does not lose the choice being made.
  function dropdownBlur(e: FocusEvent, close: () => void) {
    const next = e.relatedTarget as Node | null;
    if (next && !(e.currentTarget as HTMLElement).contains(next)) {
      close();
      // Tabbing out of the list is the third way to leave it, after the pointer
      // and Escape; all three have to drop the preview.
      endPreview();
    }
  }

  function dropdownTriggerKey(e: KeyboardEvent, open: () => void, els: HTMLButtonElement[]) {
    if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      open();
      queueMicrotask(() => els[0]?.focus());
    }
  }

  function dropdownListKey(e: KeyboardEvent, close: () => void, els: HTMLButtonElement[]) {
    if (e.key === "Escape") {
      close();
      // Leaving on Escape must also drop whatever the pointer was previewing.
      endPreview();
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = els.filter(Boolean);
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const step = e.key === "ArrowDown" ? 1 : -1;
    // Wraps at both ends: a list this long is worse to walk off than to cycle.
    items[(at + step + items.length) % items.length]?.focus();
  }

  function chooseCustomPreset(p: ThemePreset) {
    applyCustomPreset(p);
    customOpen = false;
  }

  // Closing by clicking the trigger again leaves the pointer nowhere near the
  // list, so onmouseleave never fires — the preview would stay applied.
  function toggleDropdown(open: boolean, set: (v: boolean) => void) {
    set(!open);
    if (open) endPreview();
  }

  let saving = $state(false);
  let saved = $state(false);
  // Autosave (no "Save" button). The form is loaded once and then written back on
  // every change; `loaded` keeps the initial load — and the reload after an
  // import — from being written straight back as if the user had typed it.
  let loaded = $state(false);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  // The global hotkeys as last handed to the OS, so a save that did not touch
  // them does not re-register anything.
  let lastRegisteredGlobals = $state("");

  // The same 800ms the note editor uses. Long enough that typing a word is one
  // write, short enough that leaving the screen right after a change still saves.
  function scheduleSave() {
    if (!loaded) return;
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => { saveTimer = null; void save(); }, 800);
  }

  // For a discrete choice — a preset, a checkbox, a dropdown — waiting adds
  // nothing: there is no next keystroke to coalesce with.
  function saveNow() {
    if (!loaded) return;
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
    void save();
  }
  let error: string | null = $state(null);
  let trackingMode: "extended" | "basic" | null = $state(null);
  let windowTracking: string | null = $state(null);
  let modelPath: string | null = $state(null);
  let whisperPath: string | null = $state(null);
  // The number of cleared domain records, shown after the click so the action does
  // not look like "nothing happened".
  let domainCleared: number | null = $state(null);

  async function clearDomains() {
    domainCleared = await api.clearDomainHistory().catch(() => null);
  }

  // --- Tabs: the sections are grouped so there is no single long column to scroll.
  // SECTION_TAB[i] is the tab id for the section at index i (the section indices are
  // the same ones sectionEls/sectionMatches use below). The labels go through a
  // $derived rather than a plain const: the language changes without a reload, and a
  // const would be computed once at module load and leave the tabs in the old one.
  const TAB_IDS = ["general", "ai", "tasks", "notifications", "data", "hotkeys", "help"] as const;
  type TabId = (typeof TAB_IDS)[number];
  const TABS = $derived<{ id: TabId; label: string }[]>([
    { id: "general", label: t("Общее") },
    { id: "ai", label: t("ИИ") },
    { id: "tasks", label: t("Категории") },
    { id: "notifications", label: t("Уведомления") },
    { id: "data", label: t("Данные") },
    { id: "hotkeys", label: t("Хоткеи") },
    { id: "help", label: t("Справка") },
  ]);
  // Appearance(0) and Work mode(2) -> General; AI provider(1) -> AI;
  // Monitoring(3) and Task categories(4) -> Tasks; Notifications(5) ->
  // Notifications; Auto-backup(6) and Data(7) -> Data; Hotkeys(8) -> Hotkeys;
  // Statuses(9) -> Tasks (appended last by index so the existing sections did not
  // have to be renumbered, but logically grouped with Categories); Help(10) -> Help
  // (also appended last by index for the same reason); Voice input(11) -> AI (same
  // again: appended by index, grouped with the AI provider).
  const SECTION_TAB: TabId[] = ["general", "ai", "general", "tasks", "tasks", "notifications", "data", "data", "hotkeys", "tasks", "help", "ai"];
  let activeTab = $state<TabId>(restoreOneOf(loadUiState().settingsTab, TAB_IDS, "general"));
  // Saved only while the search box is empty: a search jumps to the first matching
  // tab by itself (recomputeSearch below), and that is a transient move, not a
  // choice worth reopening the app on.
  $effect(() => {
    if (searchQuery.trim()) return;
    saveUiState({ settingsTab: activeTab });
  });

  // --- Settings search: a plain substring match over the whole text of a section,
  // with no indexing or fuzziness. An empty query shows everything everywhere; a
  // non-empty one automatically switches to the first tab with a match, and inside
  // that tab non-matching sections stay hidden.
  let searchQuery = $state("");
  let sectionEls: HTMLElement[] = $state([]);
  let sectionMatches = $state<boolean[]>([]);
  // While a search is active the help topics are expanded: otherwise the match sits
  // inside a collapsed <details> and the user sees a topic with no visible text.
  let helpSearchOpen = $derived(searchQuery.trim() !== "");

  function recomputeSearch() {
    const q = searchQuery.trim().toLowerCase();
    sectionMatches = sectionEls.map(el =>
      !q || (el?.textContent?.toLowerCase().includes(q) ?? true)
    );
    if (q) {
      const firstMatch = sectionMatches.findIndex(m => m);
      if (firstMatch >= 0) activeTab = SECTION_TAB[firstMatch];
    }
  }

  // "Window class -> category" rules: edited as rows and serialized into
  // settings.app_category_rules on save.
  let appRules: AppCategoryRule[] = $state([]);
  const RULE_CATEGORIES: { value: AppCategoryRule["category"]; label: string }[] = $derived([
    { value: "Work", label: t("Работа") },
    { value: "Study", label: t("Учёба") },
    { value: "Home", label: t("Дом") },
    { value: "Health", label: t("Здоровье") },
    { value: "Other", label: t("Другое") },
  ]);

  function parseRules(json: string): AppCategoryRule[] {
    try {
      const v = JSON.parse(json);
      return Array.isArray(v) ? v : [];
    } catch {
      return [];
    }
  }

  // --- AI suggestion of app rules ---
  //
  // Apps with no rule all land in "Other", and writing globs by hand is exactly the
  // work worth handing to a model. Suggest-then-confirm, as everywhere else here:
  // the model only proposes and the rules appear in the list by an explicit click.
  //
  // Writing them straight into the settings would silently rewrite statistics for
  // past days — the categories are applied at read time, so a wrong rule would
  // retroactively distort the dashboard.
  let ruleSuggestBusy = $state(false);
  let ruleSuggestError = $state("");
  // Suggestions with a checkbox each. Ticked by default: the usual case is accepting
  // nearly everything rather than picking items one by one.
  let ruleSuggestions: { pattern: string; category: string; take: boolean }[] = $state([]);
  // Shown when the model returned nothing to add — a distinct state from "not run
  // yet", or the button would look broken.
  let ruleSuggestEmpty = $state(false);

  async function suggestAppRules() {
    ruleSuggestBusy = true;
    ruleSuggestError = "";
    ruleSuggestEmpty = false;
    ruleSuggestions = [];
    try {
      await api.aiSuggestAppRules();
    } catch (e) {
      ruleSuggestBusy = false;
      ruleSuggestError = String(e);
    }
  }

  // Accepted rules go to the START of the list: the first match wins in
  // categorize_app, and appended at the end they would be shadowed by a broader
  // user-written pattern (say "*fox") and quietly do nothing.
  function acceptRuleSuggestions() {
    const picked = ruleSuggestions.filter(r => r.take);
    appRules = [...picked.map(r => ({ pattern: r.pattern, category: r.category })), ...appRules];
    ruleSuggestions = [];
  }

  // Time limits per app category: one entry per category, where 0 or empty means no
  // limit. Serialized into settings.app_limits on save.
  let appLimits: Record<string, number> = $state({});

  function parseLimits(json: string): AppLimit[] {
    try {
      const v = JSON.parse(json);
      return Array.isArray(v) ? v : [];
    } catch {
      return [];
    }
  }

  // The model's answer arrives as an event, like every other AI command here.
  let ruleUnlisten: UnlistenFn | null = null;
  onDestroy(() => ruleUnlisten?.());

  onMount(async () => {
    syncIsDark();
    ruleUnlisten = await listen<{ rules: AppCategoryRule[] | null; error: string | null }>(
      "ai-app-rules",
      (e) => {
        ruleSuggestBusy = false;
        if (e.payload.error) {
          ruleSuggestError = e.payload.error;
          return;
        }
        const proposed = e.payload.rules ?? [];
        // Anything already present in the list is dropped: the model may repeat a
        // rule the user added while it was thinking.
        const fresh = proposed.filter(
          p => !appRules.some(r => r.pattern.trim().toLowerCase() === p.pattern.toLowerCase()),
        );
        ruleSuggestions = fresh.map(r => ({ ...r, take: true }));
        ruleSuggestEmpty = fresh.length === 0;
      },
    );
    try {
      settings = await api.getSettings();
      // An empty setting means the language was never chosen explicitly. The select
      // shows the language actually in effect (determined by i18n.init from the
      // locale), otherwise the field would look empty while the translation works.
      if (!settings.language) settings.language = i18n.lang;
      appRules = parseRules(settings.app_category_rules);
      customPresets = parsePresets(settings.custom_theme_presets);
      appLimits = Object.fromEntries(
        parseLimits(settings.app_limits).map(l => [l.category, l.daily_mins])
      );
      keybinds = parseKeybinds(settings.keybinds);
      globalBinds = parseKeybinds(settings.global_keybinds);
      // What the OS already has registered from the previous run.
      lastRegisteredGlobals = settings.global_keybinds;
    } catch (e) {
      error = String(e);
    }
    // The list of global actions comes from the backend, which is what registers them.
    globalActions = await api.listGlobalActions().catch(() => []);
    trackingMode = await api.getTrackingMode().catch(() => null);
    windowTracking = await api.getWindowTracking().catch(() => null);
    // The real path from the backend rather than a string assembled on the
    // frontend: the directory depends on the OS (app_data_dir) and on the
    // application's identifier.
    modelPath = await api.modelPath().catch(() => null);
    whisperPath = await api.modelPath("whisper").catch(() => null);
    categoryStore.load();
    statusStore.load();
    // Only now: everything above assigns to `settings`, and autosave must not
    // write the freshly loaded values straight back.
    loaded = true;
  });

  // The autosave trigger. Reading the fields explicitly rather than relying on a
  // deep read of `settings`: a $derived over the whole object would also fire on
  // the backend-owned fields the backup loop writes (last_auto_backup), turning
  // every backup into a settings save.
  $effect(() => {
    JSON.stringify(settings);
    appRules;
    appLimits;
    keybinds;
    globalBinds;
    if (loaded) scheduleSave();
  });

  onDestroy(() => {
    // A change made in the last 800ms would otherwise be lost on leaving the
    // screen — the very case an explicit button used to cover.
    if (saveTimer) {
      clearTimeout(saveTimer);
      void save();
    }
  });

  // --- Hotkeys: the overrides live in settings.keybinds (JSON) and the defaults in
  // KEYBIND_ACTIONS.defaultCombo. A new binding is recorded by clicking "Record",
  // and the next key press that is not a modifier is captured.
  let keybinds: Keybinds = $state({});
  let recordingActionId: string | null = $state(null);
  let keybindConflict: { actionId: string; withLabel: string } | null = $state(null);

  // While recording, App.svelte does not execute hotkeys: otherwise recording a
  // combination already taken by a local action (Ctrl+K) would run that action and
  // pull focus out of the recording field.
  function setRecordingFlag(on: boolean) {
    window.dispatchEvent(new CustomEvent("keybind-recording", { detail: on }));
  }

  function startRecording(actionId: string) {
    recordingActionId = actionId;
    keybindConflict = null;
    setRecordingFlag(true);
  }

  function onKeybindCapture(e: KeyboardEvent) {
    if (!recordingActionId) return;
    e.preventDefault();
    if (e.key === "Escape") { recordingActionId = null; setRecordingFlag(false); return; }
    const combo = comboFromEvent(e);
    if (!combo) return; // only a modifier was pressed — we wait for the main key

    const conflicts = findConflicts(keybinds, recordingActionId, combo);
    if (conflicts.length > 0) {
      const other = KEYBIND_ACTIONS.find(a => a.id === conflicts[0]);
      keybindConflict = { actionId: recordingActionId, withLabel: other?.label ?? conflicts[0] };
      return;
    }
    keybinds = { ...keybinds, [recordingActionId]: combo };
    recordingActionId = null;
    keybindConflict = null;
    setRecordingFlag(false);
  }

  function resetKeybind(actionId: string) {
    const { [actionId]: _drop, ...rest } = keybinds;
    keybinds = rest;
  }

  // --- Global hotkeys ---
  //
  // The combination format matches the webview hotkeys ("Ctrl+Shift+KeyN"): it was
  // specifically verified that the global-hotkey parser understands both that form
  // and "Ctrl+Shift+N", so no converter between formats is needed and recording a
  // combination in the UI is identical for both groups.
  //
  // There are three differences from the local ones: the action list comes from the
  // backend (which also registers them), the backend validates the combination, and
  // after saving a re-registration is required — otherwise a new combination would
  // only start working after an application restart.
  let globalActions: GlobalAction[] = $state([]);
  let globalBinds: Keybinds = $state({});
  let recordingGlobalId: string | null = $state(null);
  let globalError: { actionId: string; text: string } | null = $state(null);
  // Combinations the OS refused to hand over (taken by another application or by
  // the compositor). Shown separately: this is not an input error but a fact about
  // the environment.
  let globalFailed: string[] = $state([]);

  function globalComboFor(actionId: string): string {
    const a = globalActions.find(x => x.id === actionId);
    return globalBinds[actionId] ?? a?.default_combo ?? "";
  }

  function startRecordingGlobal(actionId: string) {
    recordingGlobalId = actionId;
    globalError = null;
    setRecordingFlag(true);
  }

  async function onGlobalCapture(e: KeyboardEvent) {
    if (!recordingGlobalId) return;
    e.preventDefault();
    if (e.key === "Escape") { recordingGlobalId = null; setRecordingFlag(false); return; }
    const combo = comboFromEvent(e);
    if (!combo) return; // only a modifier — we wait for the main key

    const actionId = recordingGlobalId;

    // A conflict within the group: the OS cannot tell two global commands on one
    // combination apart.
    const dupe = globalActions.find(a => a.id !== actionId && globalComboFor(a.id) === combo);
    if (dupe) {
      globalError = { actionId, text: t("Уже занято: {label}", { label: dupe.label }) };
      return;
    }
    // A conflict with a local hotkey: the global one intercepts keys first, so the
    // local one would simply stop working — silently and inexplicably.
    const localDupe = KEYBIND_ACTIONS.find(a => comboFor(keybinds, a.id) === combo);
    if (localDupe) {
      globalError = { actionId, text: t("Занято хоткеем в приложении: {label}", { label: localDupe.label }) };
      return;
    }
    // The final say belongs to the real combination parser rather than to our own
    // rules: it is the one that will do the registering.
    try {
      await api.validateGlobalCombo(combo);
    } catch (err) {
      // While the answer was awaited the user may have left recording (Escape) or
      // started recording another action, in which case the reply is stale and its
      // error must not be shown: doing so would put the field back into recording mode.
      if (recordingGlobalId !== actionId) return;
      globalError = { actionId, text: typeof err === "string" ? err : t("Комбинация не подходит") };
      return;
    }
    if (recordingGlobalId !== actionId) return;

    globalBinds = { ...globalBinds, [actionId]: combo };
    recordingGlobalId = null;
    globalError = null;
    setRecordingFlag(false);
  }

  function resetGlobalKeybind(actionId: string) {
    const { [actionId]: _drop, ...rest } = globalBinds;
    globalBinds = rest;
  }

  // --- Task categories (CRUD is saved immediately, with no "Save" button) ---
  let newCatName = $state("");
  let newCatColor = $state("#2a78d6");

  async function addCategory() {
    const name = newCatName.trim();
    if (!name) return;
    await categoryStore.create(name, newCatColor);
    newCatName = "";
  }

  // --- Task statuses (for the kanban board), following the same pattern as
  // categories: CRUD is saved immediately, with no "Save" button.
  // Todo/InProgress/Done/Archived are reserved (is_reserved) and can be neither
  // renamed nor deleted.
  let newStatusName = $state("");
  let newStatusColor = $state("#2a78d6");

  async function addStatus() {
    const name = newStatusName.trim();
    if (!name) return;
    await statusStore.create(name, newStatusColor);
    newStatusName = "";
  }

  async function save() {
    saving = true;
    error = null;
    try {
      settings.app_category_rules = JSON.stringify(appRules.filter(r => r.pattern.trim()));
      settings.app_limits = JSON.stringify(
        Object.entries(appLimits)
          .filter(([, mins]) => mins > 0)
          .map(([category, daily_mins]) => ({ category, daily_mins }))
      );
      settings.keybinds = JSON.stringify(keybinds);
      settings.global_keybinds = JSON.stringify(globalBinds);
      await api.saveSettings(settings);
      // Re-registration with the OS: without it a new combination would only start
      // working after a restart while the old one kept firing. Under autosave it
      // is conditional — every keystroke in an unrelated field used to drag the
      // OS-level hotkey registry along with it.
      if (settings.global_keybinds !== lastRegisteredGlobals) {
        globalFailed = await api.applyGlobalHotkeys().catch(() => []);
        lastRegisteredGlobals = settings.global_keybinds;
      }
      applyTheme(settings.theme_mode, settings);
      // App.svelte keeps its own copy of the hotkeys for the keydown handler —
      // without this event a rebinding would only take effect after a reload.
      window.dispatchEvent(new CustomEvent("keybinds-saved", { detail: settings.keybinds }));
      saved = true;
      setTimeout(() => saved = false, 2000);
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  let backupMsg: string | null = $state(null);
  let backupNowBusy = $state(false);
  let backupNowMsg = $state("");
  // last_auto_backup is written by the backend and never sent back (see the field
  // comment in commands/settings.rs), so it is read straight off the loaded
  // settings rather than kept in its own state. Until v0.9.85 a `lastBackup`
  // variable was declared here, rendered below, and assigned nowhere — the line
  // had never once appeared.
  // Date and time both matter here: with a 24h cycle, a bare date cannot tell
  // "this morning" from "just before midnight yesterday". Months and weekdays go
  // through Intl rather than the dictionary, as everywhere else in the app.
  function fmtBackupDate(iso: string): string {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString(localeTag(i18n.lang), { dateStyle: "medium", timeStyle: "short" });
  }

  let backupLevel = $derived(
    backupHealth(
      settings.auto_backup_dir ?? "",
      settings.last_auto_backup ?? "",
      new Date(),
      settings.last_auto_backup_error ?? "",
    ),
  );

  // The stored form is "<rfc3339>\t<message>" (backup.rs::run_auto_backup); only
  // the message is worth showing, the date of the failure adds nothing next to a
  // warning that is live right now.
  let backupErrorMsg = $derived(
    (settings.last_auto_backup_error ?? "").split("\t").slice(1).join("\t"),
  );

  async function pickBackupDir() {
    error = null;
    try {
      const path = await openDialog({ directory: true, multiple: false });
      if (path) settings.auto_backup_dir = path;
    } catch (e) {
      error = String(e);
    }
  }

  async function doBackupNow() {
    backupNowBusy = true;
    backupNowMsg = "";
    try {
      const name = await api.doAutoBackup();
      backupNowMsg = t("Бэкап сохранён: {name}", { name });
    } catch (e) {
      backupNowMsg = t("Ошибка: {e}", { e: String(e) });
    } finally {
      backupNowBusy = false;
    }
  }

  async function exportData() {
    backupMsg = null;
    error = null;
    try {
      const path = await saveDialog({
        defaultPath: "kriptag-backup.zip",
        filters: [{ name: "ZIP", extensions: ["zip"] }],
      });
      if (!path) return;
      await api.exportData(path);
      backupMsg = t("Экспорт завершён ✓");
    } catch (e) {
      error = String(e);
    }
  }

  // A test button: reset the onboarding and reload the webview so App.svelte
  // re-reads the settings and shows the onboarding straight away. We take fresh
  // settings from the DB so unsaved form edits are not written along with it.
  async function resetOnboarding() {
    error = null;
    try {
      const fresh = await api.getSettings();
      fresh.onboarding_complete = false;
      await api.saveSettings(fresh);
      location.reload();
    } catch (e) {
      error = String(e);
    }
  }

  // The archive is inspected first and only then confirmed (v0.9.92). Until this
  // version the confirmation came BEFORE the file was even chosen — the user
  // agreed to "replace all current data" without knowing which copy, and the file
  // dialog shows nothing but a timestamp in the name. Testing on a real install
  // produced the same mistake twice within an hour: an older snapshot picked, a
  // note created after it silently rolled back.
  async function importData() {
    backupMsg = null;
    error = null;
    try {
      const path = await openDialog({
        multiple: false,
        filters: [{ name: "ZIP", extensions: ["zip"] }],
      });
      if (!path) return;

      const p = await api.previewImport(path as string);
      const at = p.newest ? fmtBackupDate(p.newest) : t("неизвестно");
      const losing = p.losing_tasks + p.losing_notes;

      // Both sides, so a count has something to be compared against: "39 tasks"
      // alone says nothing about whether this copy is ahead or behind.
      const delta = (from: number, to: number) =>
        to === from ? "" : ` (${to > from ? "+" : "−"}${Math.abs(to - from)})`;

      let question = t("Копия от {d}", { d: at }) + "\n"
        + t("Задачи: {a} → {b}{d}", {
            a: String(p.current_tasks), b: String(p.tasks),
            d: delta(p.current_tasks, p.tasks),
          }) + "\n"
        + t("Заметки: {a} → {b}{d}", {
            a: String(p.current_notes), b: String(p.notes),
            d: delta(p.current_notes, p.notes),
          });

      // The line that actually stops the mistake. Everything above is context;
      // this is the consequence.
      if (losing > 0) {
        question += "\n\n" + t("ВНИМАНИЕ: в текущей базе новее этой копии — задач: {t}, заметок: {n}. Они будут потеряны безвозвратно.", { t: String(p.losing_tasks), n: String(p.losing_notes) });
      }
      question += "\n\n" + t("Импорт заменит все текущие данные, приложение закроется. Продолжить?");

      // GTK always draws a heading above the text, with or without `title` — an
      // omitted one just becomes the app name, so it showed "kriptag" twice.
      // Giving it a real title is the only way to make that line useful.
      // Button labels are explicit too: the plugin defaults to English
      // "Ok"/"Cancel" regardless of the interface language.
      const go = await confirmDialog(question, {
        title: t("Импорт данных"),
        kind: "warning",
        okLabel: t("Импортировать"),
        cancelLabel: t("Отмена"),
      });
      if (!go) return;
      // api.importData never returns: the backend calls app.restart(), which is
      // typed `-> !`. Nothing may be placed after it — see the note below the
      // Import button, which is why the user is warned in the dialog instead.
      await api.importData(path as string);
    } catch (e) {
      error = String(e);
    }
  }

  let notesMdMsg = $state("");

  async function exportNotesMd() {
    notesMdMsg = "";
    error = null;
    try {
      const dir = await openDialog({ directory: true, multiple: false });
      if (!dir) return;
      const count = await api.exportNotesMd(dir as string);
      notesMdMsg = t("Экспортировано заметок: {n}", { n: count });
    } catch (e) {
      error = String(e);
    }
  }

  async function importNotesMd() {
    notesMdMsg = "";
    error = null;
    try {
      const dir = await openDialog({ directory: true, multiple: false });
      if (!dir) return;
      const count = await api.importNotesMd(dir as string);
      notesMdMsg = t("Импортировано заметок: {n}. Совпадения по названию создаются как отдельные заметки.", { n: count });
    } catch (e) {
      error = String(e);
    }
  }
</script>

<div class="settings">
  <h2 class="page-title" style="margin-bottom:14px;">{t("Настройки")}</h2>

  <input
    type="search"
    class="settings-search"
    placeholder={t("Поиск по настройкам…")}
    bind:value={searchQuery}
    oninput={recomputeSearch}
  />

  <!-- seg + seg--underline provide the look; .settings-tabs remains for its own
       spacing and its wrap onto a second line. The .settings-tab name is left
       alone: about 25 e2e tests select by it, and this change is purely
       cosmetic. -->
  <div class="settings-tabs seg seg--underline" role="tablist">
    {#each TABS as tab (tab.id)}
      <button
        type="button"
        class="settings-tab"
        class:active={activeTab === tab.id}
        role="tab"
        aria-selected={activeTab === tab.id}
        onclick={() => activeTab = tab.id}
      >{tab.label}</button>
    {/each}
  </div>

  {#if error}
    <div class="alert">{error}</div>
  {/if}

  <section class="card panel" class:hidden-by-search={sectionMatches[0] === false} class:hidden-by-tab={SECTION_TAB[0] !== activeTab} bind:this={sectionEls[0]}>
    <h3 class="section-title">{t("Внешний вид")}</h3>

    <!-- Language: applied immediately, without "Save", just like the theme. For
         the language that matters more than for the theme: seeing the result
         before saving is the only way to tell you picked the right one. -->
    <label class="field">
      {t("Язык")}
      <Select
        value={settings.language}
        ariaLabel={t("Язык")}
        onChange={(v) => { settings!.language = v; i18n.set(v as Lang); }}
        options={LANGS.map(l => ({ value: l.id, label: l.label }))}
      />
    </label>

    <!-- The caption is what tells the toggle apart from the language field above
         it. Spacing alone was not enough: everything else in this panel is
         labelled ("Язык" above, "Пресеты акцента" below), so an unlabelled row of
         buttons read as a continuation of the field before it.

         .sub-label, the same element the accent presets use — a caption for a
         control that is not a <label>-wrapped input. -->
    <div class="sub-label theme-label">{t("Тема")}</div>
    <!-- .seg rather than radio buttons: this is a three-way toggle, which is
         exactly what the segmented control is for, and native radios are drawn by
         the GTK theme in WebKitGTK — round, in system colours, past the tokens.
         The same reason the selects and the subtask ticks had to be replaced. -->
    <div class="seg theme-seg" role="radiogroup" aria-label={t("Тема")}>
      {#each [["light", t("Светлая")], ["dark", t("Тёмная")], ["system", t("Системная")]] as [val, label] (val)}
        <button
          type="button"
          role="radio"
          aria-checked={settings.theme_mode === val}
          class:active={settings.theme_mode === val}
          onclick={() => { settings!.theme_mode = val as AppSettings["theme_mode"]; switchThemeMode(); }}
        >{label}</button>
      {/each}
    </div>

    <div class="sub-label">{t("Пресеты акцента")}</div>
    <!-- A listbox rather than a native <select>: the gradient swatch is half of
         what a preset says, and <option> cannot carry one. Closing on focusout
         covers both a click elsewhere and tabbing away, with no global listener
         to leak. -->
    <div class="preset-select" role="none" onfocusout={(e) => dropdownBlur(e, () => presetOpen = false)}
         onkeydown={(e) => { if (e.key === "Escape" && presetOpen) { presetOpen = false; endPreview(); } }}>
      <button
        type="button"
        class="preset-trigger"
        aria-haspopup="listbox"
        aria-expanded={presetOpen}
        onclick={() => toggleDropdown(presetOpen, (v) => presetOpen = v)}
        onkeydown={(e) => dropdownTriggerKey(e, () => presetOpen = true, presetOptionEls)}
      >
        <span class="swatch" style="background:{currentPresetSwatch};"></span>
        <span class="preset-trigger-name">{currentPresetName}</span>
        <span class="preset-caret" aria-hidden="true">▾</span>
      </button>

      {#if presetOpen}
        <!-- Hover and keyboard focus both preview: the list is walkable by
             arrows, and a preview only the mouse can reach would be a different
             control depending on how you got there. -->
        <ul class="preset-list" role="listbox" tabindex="-1"
            onkeydown={(e) => dropdownListKey(e, () => presetOpen = false, presetOptionEls)}
            onmouseleave={endPreview}>
          {#each THEME_PRESETS as p, i}
            <li>
              <button
                type="button"
                role="option"
                aria-selected={p.name === currentPresetName}
                class="preset-option"
                class:selected={p.name === currentPresetName}
                bind:this={presetOptionEls[i]}
                onmouseenter={() => previewPreset(p)}
                onfocus={() => previewPreset(p)}
                onclick={() => choosePreset(p)}
              >
                <span class="swatch" style="background:linear-gradient(135deg, {p.accent}, {p.accentSecondary});"></span>
                {p.name}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <!-- Saved sets. Shown below the built-in ones and only once something has
         been saved: an empty list with a permanent name field would read as an
         unfinished form. -->
    {#if customPresets.length > 0}
      <div class="sub-label" style="margin-top:12px;">{t("Мои пресеты")}</div>
      <div class="preset-select" role="none" onfocusout={(e) => dropdownBlur(e, () => customOpen = false)}
           onkeydown={(e) => { if (e.key === "Escape" && customOpen) { customOpen = false; endPreview(); } }}>
        <button
          type="button"
          class="preset-trigger"
          aria-haspopup="listbox"
          aria-expanded={customOpen}
          onclick={() => toggleDropdown(customOpen, (v) => customOpen = v)}
          onkeydown={(e) => dropdownTriggerKey(e, () => customOpen = true, customOptionEls)}
        >
          <span class="swatch" style="background:{customPresets.length ? presetSwatch(customPresets[0]) : 'transparent'};"></span>
          <span class="preset-trigger-name">{t("Выбрать набор")}</span>
          <span class="preset-caret" aria-hidden="true">▾</span>
        </button>

        {#if customOpen}
          <ul class="preset-list" role="listbox" tabindex="-1"
              onkeydown={(e) => dropdownListKey(e, () => customOpen = false, customOptionEls)}
              onmouseleave={endPreview}>
            {#each customPresets as p, i (p.name)}
              <li class="custom-option">
                <button
                  type="button"
                  role="option"
                  aria-selected="false"
                  class="preset-option"
                  bind:this={customOptionEls[i]}
                  onmouseenter={() => previewCustomPreset(p)}
                  onfocus={() => previewCustomPreset(p)}
                  onclick={() => chooseCustomPreset(p)}
                >
                  <span class="swatch" style="background:{presetSwatch(p)};"></span>
                  {p.name}
                </button>
                <!-- Deletion lives in the row rather than behind a second click:
                     a saved set is the user's own, and pruning the list is as
                     ordinary here as picking from it. -->
                <button
                  type="button"
                  class="option-del"
                  title={t("Удалить пресет")}
                  aria-label={t("Удалить пресет")}
                  onclick={() => deleteCustomPreset(p.name)}
                >✕</button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}

    <div class="preset-row" style="margin-top:8px;">
      <input
        type="text"
        class="preset-name"
        maxlength={MAX_NAME_LEN}
        placeholder={t("Название набора")}
        bind:value={presetName}
        onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); saveCurrentAsPreset(); } }}
      />
      <button
        type="button"
        class="btn-sm"
        disabled={!presetName.trim() || (customPresets.length >= MAX_PRESETS && !customPresets.some(p => p.name === presetName.trim()))}
        onclick={saveCurrentAsPreset}
      >{t("Сохранить текущие цвета")}</button>
    </div>

    <div class="color-grid">
      {#each MAIN_COLORS as [key, label]}
        <label class="check">
          <input type="color"
            value={colorSwatch(key, (settings as any)[key] ?? "", isDark, settings.color_bg)}
            oninput={(e) => { (settings as any)[key] = e.currentTarget.value; previewTheme(); }}
            class="color-input"
            class:is-default={isDefaultColor((settings as any)[key] ?? "")} />
          {label}
        </label>
      {/each}
    </div>

    <p class="hint">{t("Сайдбар, карточки, границы и текст выводятся из фона — они всегда согласованы между собой.")}</p>

    <!-- The way back from a chosen ground. Without it the only exit is the reset
         inside the disclosure, and someone whose light theme looks wrong after a
         dark preset has no reason to look there. -->
    {#if !isDefaultColor(settings.color_bg)}
      <button type="button" class="btn-sm" style="margin-top:8px;" onclick={resetBackground}>
        {t("Вернуть фон темы")}
      </button>
    {/if}

    <!-- The disclosure holds the overrides of the derived stack. Text and
         borders in particular: breaking either one costs readability outright,
         and unlike the accent or the background they are rarely what someone
         came here to change. -->
    <details class="advanced-colors">
      <summary>{t("Продвинутые настройки цвета")}</summary>
      <p class="hint" style="margin-top:8px;">{t("Пунктир — значение выводится из фона. Заданный цвет заменяет выведенное только для этого элемента.")}</p>

      {#each ADVANCED_GROUPS as group}
        <div class="sub-label" style="margin-top:12px;">{group.title}</div>
        <div class="color-grid">
          {#each group.colors as [key, label]}
            <label class="check">
              <input type="color"
                value={colorSwatch(key, (settings as any)[key] ?? "", isDark, settings.color_bg)}
                oninput={(e) => { (settings as any)[key] = e.currentTarget.value; previewTheme(); }}
                class="color-input"
                class:is-default={isDefaultColor((settings as any)[key] ?? "")} />
              {label}
              <!-- Per-field reset: without it the only way back from one bad pick
                   is to clear every colour at once. -->
              {#if !isDefaultColor((settings as any)[key] ?? "")}
                <button type="button" class="unset-btn"
                  title={t("Вернуть выведенное значение")}
                  aria-label={t("Вернуть выведенное значение")}
                  onclick={() => { (settings as any)[key] = ""; previewTheme(); }}>✕</button>
              {/if}
            </label>
          {/each}
        </div>
      {/each}

      <button type="button" class="btn-sm" style="margin-top:12px;" onclick={resetColors}>{t("Сбросить к дефолту")}</button>
    </details>

    <label class="check" style="margin-top:12px;">
      <Switch bind:checked={settings.show_subtasks_expanded} />{t("Показывать подзадачи в списке задач развёрнутыми")}</label>
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[1] === false} class:hidden-by-tab={SECTION_TAB[1] !== activeTab} bind:this={sectionEls[1]}>
    <h3 class="section-title">{t("ИИ-провайдер")}</h3>

    <label class="field">
      <span class="label">{t("Провайдер")}</span>
      <Select
        value={settings.ai_provider}
        ariaLabel={t("Провайдер")}
        onChange={(v) => (settings!.ai_provider = v as AppSettings["ai_provider"])}
        options={PROVIDERS.map(p => ({ value: p.value, label: p.label }))}
      />
    </label>

    {#if settings.ai_provider !== "none"}
      <label class="check" style="margin-top:10px;">
        <Switch bind:checked={settings.ai_fallback} />{t("Автопереключение: при ошибке или недоступности пробовать других доступных провайдеров")}</label>
    {/if}

    <!-- One settings block whose fields depend on the chosen provider, not two
         parallel duplicating blocks as there were with the radio list. -->
    {#if settings.ai_provider === "openai" || settings.ai_provider === "anthropic"}
      {@const isOpenai = settings.ai_provider === "openai"}
      <div class="stack" style="margin-top:12px;">
        <label class="field">
          <span class="label">API Key
            {#if isOpenai ? settings.openai_key : settings.anthropic_key}
              {#if isOpenai ? settings.openai_in_keyring : settings.anthropic_in_keyring}
                <span class="key-ok"><Icon name="lock" size={11} /> keyring</span>
              {:else}
                <span class="key-warn">{t("⚠ БД (keyring недоступен)")}</span>
              {/if}
            {/if}
          </span>
          {#if isOpenai}
            <input type="password" bind:value={settings.openai_key} placeholder="sk-..." />
          {:else}
            <input type="password" bind:value={settings.anthropic_key} placeholder="sk-ant-..." />
          {/if}
        </label>
        <label class="field">
          <span class="label">{t("Модель")}</span>
          {#if isOpenai}
            <Select
              value={settings.openai_model}
              ariaLabel={t("Модель")}
              onChange={(v) => (settings!.openai_model = v)}
              options={[
                { value: "gpt-4o-mini", label: t("gpt-4o-mini (быстрый, дешёвый)") },
                { value: "gpt-4o", label: "gpt-4o" },
                { value: "gpt-4-turbo", label: "gpt-4-turbo" },
              ]}
            />
          {:else}
            <Select
              value={settings.anthropic_model}
              ariaLabel={t("Модель")}
              onChange={(v) => (settings!.anthropic_model = v)}
              options={[
                { value: "claude-haiku-4-5-20251001", label: t("claude-haiku-4-5 (быстрый, дешёвый)") },
                { value: "claude-sonnet-4-6", label: "claude-sonnet-4-6" },
              ]}
            />
          {/if}
        </label>
      </div>
    {:else if settings.ai_provider === "local"}
      <div style="margin-top:12px;">
        <p class="muted" style="font-size:12px;margin:0 0 10px 0;">{t("Локальная модель хранится в")}<code>{modelPath ?? "…"}</code>
        </p>
        <ModelDownloader />
      </div>
    {/if}
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[2] === false} class:hidden-by-tab={SECTION_TAB[2] !== activeTab} bind:this={sectionEls[2]}>
    <h3 class="section-title">{t("Режим работы")}</h3>
    <Select
      value={settings.work_mode}
      ariaLabel={t("Режим работы")}
      onChange={(v) => (settings!.work_mode = v as AppSettings["work_mode"])}
      options={[
        { value: "Light", label: t("Light — обычный режим") },
        { value: "Focus", label: t("Focus — без уведомлений") },
        { value: "Study", label: t("Study — помодоро-сессии (25/5)") },
      ]}
    />
    <p class="hint">{t("Применяется сразу после сохранения.")}</p>

    {#if settings.work_mode === "Study"}
      <div class="pair" style="margin-top:10px;">
        <label class="field">
          <span class="label">{t("Рабочий блок (мин)")}</span>
          <input type="number" min="1" max="120" bind:value={settings.pomodoro_work_mins} />
        </label>
        <label class="field">
          <span class="label">{t("Перерыв (мин)")}</span>
          <input type="number" min="1" max="60" bind:value={settings.pomodoro_break_mins} />
        </label>
      </div>
      <p class="hint">{t("Применяется при следующем входе в режим Study.")}</p>
    {/if}
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[3] === false} class:hidden-by-tab={SECTION_TAB[3] !== activeTab} bind:this={sectionEls[3]}>
    <h3 class="section-title">{t("Мониторинг")}</h3>
    <div class="pair">
      <label class="field">
        <span class="label">{t("Порог простоя (сек, мин. 60)")}</span>
        <input type="number" min="60" bind:value={settings.idle_threshold_secs} />
      </label>
      <label class="field">
        <span class="label">{t("Интервал логирования (сек, 10–600)")}</span>
        <input type="number" min="10" max="600" bind:value={settings.log_interval_secs} />
      </label>
    </div>
    <p class="hint">{t("Применяется после перезапуска приложения.")}</p>
    {#if trackingMode}
      <p class="hint">
        {t("Режим трекинга")}: {trackingMode === "extended"
          ? t("расширенный — системный простой/возврат от композитора (ext-idle-notify)")
          : t("базовый — только ввод в окне приложения")}
        {windowTracking ? ` · ${t("приложения")}: ${windowTracking}` : ""}
      </p>
    {/if}

    <!-- Domains: shown in the same place window tracking works — without a
         provider there is nowhere to read a title from and the checkbox would be
         dead. The wording is deliberately blunt: the user must understand what
         will actually start happening rather than see an innocuous "improve the
         statistics". -->
    {#if windowTracking}
      <label class="option" style="margin-top:12px;align-items:flex-start;">
        <Switch bind:checked={settings.track_domains} />
        <span>{t("Разбивать браузерное время по сайтам")}
          <br /><small class="hint" style="margin:0;">
            {t("Требует чтения заголовков окон браузера. В базу сохраняется")}
            <b>{t("только домен")}</b> {t("(github.com), сам заголовок — название вкладки, поисковый запрос — не сохраняется никогда. Выключено по умолчанию.")}
          </small>
        </span>
      </label>
      {#if domainCleared !== null}
        <p class="hint">{t("Очищено записей: {n}", { n: domainCleared })}</p>
      {/if}
      <button class="btn-sm" style="margin-top:6px;" onclick={clearDomains}>{t("Забыть собранные домены")}</button>

      <div class="sub-label" style="margin-top:12px;">{t("Категории приложений (класс окна → категория)")}</div>
      {#each appRules as rule, i}
        <div class="rule-row">
          <input bind:value={rule.pattern} placeholder={t("класс окна, напр. jetbrains-*")} />
          <span class="rule-cat">
            <Select
              value={rule.category}
              ariaLabel={t("Категория")}
              onChange={(v) => (rule.category = v)}
              options={RULE_CATEGORIES.map(c => ({ value: c.value, label: c.label }))}
            />
          </span>
          <button class="btn-icon btn-danger" title={t("Удалить правило")}
            onclick={() => appRules = appRules.filter((_, j) => j !== i)}>✕</button>
        </div>
      {/each}
      <button class="btn-sm" onclick={() => appRules = [...appRules, { pattern: "", category: "Work" }]}>{t("+ Правило")}</button>
 <p class="hint">{t("Первое совпавшее правило выигрывает;")}<code>*</code>{t("— любая подстрока. Приложения без правила попадают в «Другое». Применяется после «Сохранить».")}</p>

      <!-- Suggest-then-confirm: the model proposes, the rules appear in the list
           above only by an explicit click. Hidden when AI is off — the same
           capability detection as the other AI buttons. -->
      {#if settings.ai_provider !== "none"}
        <button class="btn-sm" style="margin-top:6px;" onclick={suggestAppRules} disabled={ruleSuggestBusy}>
          {ruleSuggestBusy ? t("Определяю…") : t("Определить категории через ИИ")}
        </button>
        {#if ruleSuggestError}
          <span class="alert" style="margin-top:6px;">{tErr(ruleSuggestError)}</span>
        {/if}
        {#if ruleSuggestEmpty}
          <p class="hint">{t("Все приложения из статистики уже покрыты правилами.")}</p>
        {/if}
        {#if ruleSuggestions.length > 0}
          <div class="rule-suggestions">
            {#each ruleSuggestions as sug (sug.pattern)}
              <!-- Stays a checkbox rather than becoming a Switch: this ticks a row
                   in a list ("Добавить отмеченные"), which is a selection, not an
                   on/off setting. -->
              <label class="rule-row suggestion-row">
                <input type="checkbox" bind:checked={sug.take} />
                <code style="flex:1;">{sug.pattern}</code>
                <span class="muted">{RULE_CATEGORIES.find(c => c.value === sug.category)?.label ?? sug.category}</span>
              </label>
            {/each}
            <button class="btn-sm" onclick={acceptRuleSuggestions}
              disabled={!ruleSuggestions.some(r => r.take)}>
              {t("Добавить отмеченные")}
            </button>
          </div>
        {/if}
      {/if}

      <div class="sub-label" style="margin-top:12px;">{t("Лимиты времени на категории (мин/день)")}</div>
      {#each RULE_CATEGORIES as c}
        <div class="rule-row limit-row">
          <span class="muted" style="flex:1;">{c.label}</span>
          <input
            type="number" min="0" style="width:90px;"
            placeholder={t("без лимита")}
            value={appLimits[c.value] || ""}
            oninput={(e) => {
              const n = parseInt((e.currentTarget as HTMLInputElement).value, 10);
              appLimits = { ...appLimits, [c.value]: Number.isFinite(n) ? n : 0 };
            }}
          />
        </div>
      {/each}
 <p class="hint">{t("0 или пусто — без лимита. При превышении — уведомление раз в день (пока лимит остаётся превышенным). Применяется после «Сохранить».")}</p>
    {/if}
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[4] === false} class:hidden-by-tab={SECTION_TAB[4] !== activeTab} bind:this={sectionEls[4]}>
    <h3 class="section-title">{t("Категории задач")}</h3>
    {#each categoryStore.categories as c (c.id)}
      <div class="rule-row">
        <input
          type="color"
          class="cat-color"
          value={c.color}
          title={t("Цвет категории")}
          onchange={(e) => categoryStore.update(c.id, { color: e.currentTarget.value })}
        />
        <!--
          We show the translated name, but that makes the seeded categories
          uneditable: the field is bound to the same value that goes into the DB,
          and the translation would overwrite the Russian original for good. The
          same approach the statuses below take with is_reserved. Categories have
          no such flag; a seeded one is recognized by its Latin id (user-defined
          ones get a uuid).
        -->
        <input
          value={categoryStore.name(c.id)}
          disabled={SEEDED_CATEGORY_IDS.has(c.id)}
          title={SEEDED_CATEGORY_IDS.has(c.id) ? t("Встроенная категория — название нельзя менять") : ""}
          onchange={(e) => {
            const name = e.currentTarget.value.trim();
            if (name && name !== c.name) categoryStore.update(c.id, { name });
            else e.currentTarget.value = c.name;
          }}
        />
        {#if c.id !== "Other"}
          <button class="btn-icon btn-danger" title={t("Удалить (задачи перейдут в «Другое»)")}
            onclick={() => categoryStore.remove(c.id)}>✕</button>
        {:else}
          <span class="hint" style="margin:0;">{t("фолбэк")}</span>
        {/if}
      </div>
    {/each}
    <div class="rule-row">
      <input type="color" class="cat-color" bind:value={newCatColor} title={t("Цвет новой категории")} />
      <input bind:value={newCatName} placeholder={t("Новая категория")}
        onkeydown={(e) => { if (e.key === "Enter") addCategory(); }} />
      <button class="btn-sm" onclick={addCategory} disabled={!newCatName.trim()}>{t("Добавить")}</button>
    </div>
    {#if categoryStore.error}
      <p class="hint" style="color:var(--danger, #d33);">{tErr(categoryStore.error)}</p>
    {/if}
    <p class="hint">{t("Изменения сохраняются сразу. При удалении категории её задачи переходят в «Другое».")}</p>
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[5] === false} class:hidden-by-tab={SECTION_TAB[5] !== activeTab} bind:this={sectionEls[5]}>
    <h3 class="section-title">{t("Уведомления")}</h3>
    <div class="pair">
      <label class="field">
        <span class="label">{t("Первое предупреждение (часов до дедлайна)")}</span>
        <input type="number" min="1" bind:value={settings.deadline_warn_hours} />
      </label>
      <label class="field">
        <span class="label">{t("Второе предупреждение (минут до дедлайна)")}</span>
        <input type="number" min="1" max="1440" bind:value={settings.deadline_warn_minutes} />
      </label>
      <label class="field">
        <span class="label">{t("Возврат после простоя (мин, мин. 1)")}</span>
        <input type="number" min="1" bind:value={settings.idle_notify_min_mins} />
      </label>
      <label class="field">
        <span class="label">{t("Перерыв после N минут работы (0 — выкл)")}</span>
        <input type="number" min="0" bind:value={settings.nudge_after_mins} />
      </label>
    </div>
    <label class="check" style="margin-top:10px;">
      <Switch bind:checked={settings.context_notifications} />{t("Контекстные уведомления (накопились просрочки, возврат к задаче «в работе»)")}</label>
    <label class="check" style="margin-top:6px;">
      <Switch bind:checked={settings.focus_mode_auto} />{t("Фокус-режим: авто-пауза уведомлений на время помодоро-работы и активных тайм-блоков")}</label>
    <label class="field" style="margin-top:8px;">
      <span class="label">{t("Утренняя сводка (HH:MM, пусто = выкл)")}</span>
      <input type="time" bind:value={settings.morning_digest_time} />
    </label>
    <p class="hint">{t("Пауза всех уведомлений — в меню трея: «Пауза уведомлений» (30 мин / 1 ч / 2 ч / бессрочно).")}</p>
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[6] === false} class:hidden-by-tab={SECTION_TAB[6] !== activeTab} bind:this={sectionEls[6]}>
    <h3 class="section-title">{t("Авто-бэкап")}</h3>
    <div class="stack">
      <label class="field">
        <span class="label">{t("Папка для бэкапов (пусто = выкл)")}</span>
        <div class="input-row">
          <input type="text" bind:value={settings.auto_backup_dir} placeholder={t("Выберите папку...")} readonly style="flex:1;" />
          <button class="btn-sm" onclick={pickBackupDir}>{t("Обзор…")}</button>
        </div>
      </label>
      <label class="field">
        <span class="label">{t("Хранить копий")}</span>
        <input type="number" min="1" bind:value={settings.auto_backup_keep} />
      </label>
      {#if backupLevel === "error"}
        <p class="hint hint-warn">
          {t("Последний авто-бэкап не удался: {e}", { e: tErr(backupErrorMsg) })}
        </p>
      {:else if backupLevel === "off"}
        <p class="hint hint-warn">{t("Авто-бэкап выключен: папка не выбрана.")}</p>
      {:else if backupLevel === "pending"}
        <p class="hint">{t("Папка выбрана, первая копия появится в течение суток.")}</p>
      {:else if backupLevel === "stale"}
        <p class="hint hint-warn">
          {t("Последняя копия старше двух суток: {d}", { d: fmtBackupDate(settings.last_auto_backup) })}
        </p>
      {:else}
        <p class="hint">{t("Последний бэкап: {d}", { d: fmtBackupDate(settings.last_auto_backup) })}</p>
      {/if}
      <div class="preset-row">
        <button class="btn-sm" onclick={doBackupNow} disabled={backupNowBusy || !settings.auto_backup_dir.trim()}>
          {backupNowBusy ? "…" : t("Сделать сейчас")}
        </button>
        {#if backupNowMsg}
          <span class="muted" style="font-size:12px;">{backupNowMsg}</span>
        {/if}
      </div>
    </div>
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[7] === false} class:hidden-by-tab={SECTION_TAB[7] !== activeTab} bind:this={sectionEls[7]}>
    <h3 class="section-title">{t("Данные")}</h3>
    <div class="preset-row">
      <button class="btn-sm" onclick={exportData}>{t("Экспорт (ZIP)")}</button>
      <button class="btn-sm" onclick={importData}>{t("Импорт (ZIP)")}</button>
      <button class="btn-sm" onclick={resetOnboarding} title={t("Сбросит флаг onboarding_complete и покажет онбординг заново")}>{t("Сбросить онбординг")}</button>
      {#if backupMsg}
        <span class="muted" style="font-size:12px;">{backupMsg}</span>
      {/if}
    </div>
    <!-- The import ends the process (app.restart), so no "done" message can ever
         be shown after it — saying so up front is the only honest option. -->
    <p class="hint">{t("При импорте приложение закроется — откройте его заново, данные будут заменены.")}</p>
    <div class="preset-row" style="margin-top:8px;">
      <button class="btn-sm" onclick={exportNotesMd}>{t("Экспорт заметок (.md)")}</button>
      <button class="btn-sm" onclick={importNotesMd}>{t("Импорт заметок из папки")}</button>
      {#if notesMdMsg}
        <span class="muted" style="font-size:12px;">{notesMdMsg}</span>
      {/if}
    </div>
    <p class="hint">{t("Экспорт .md — для переноса в Obsidian, а не резервная копия: теги, связи, закрепление и даты не сохраняются. Полная копия — «Экспорт (ZIP)».")}</p>
    <label class="field" style="margin-top:12px;max-width:280px;">
      <span class="label">{t("Авто-очистка истории (мес., 0 — выкл)")}</span>
      <input type="number" min="0" bind:value={settings.history_cleanup_months} />
    </label>
 <p class="hint">{t("Выполненные задачи старше указанного срока автоматически переносятся в Корзину (не удаляются насовсем — статистика дашборда не страдает, т.к. дата выполнения не стирается). Проверяется раз в сутки.")}</p>
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[8] === false} class:hidden-by-tab={SECTION_TAB[8] !== activeTab} bind:this={sectionEls[8]}>
    <h3 class="section-title">{t("Хоткеи")}</h3>

    <!-- The global ones form a separate group above the local ones. The order is
         not cosmetic: they intercept keys before anything else, so a conflict
         with them explains why a local hotkey "stopped working". -->
    <h4 class="keybind-group">{t("Глобальные — работают, даже когда окно закрыто")}</h4>
    <div class="keybind-list">
      {#each globalActions as action (action.id)}
        <div class="keybind-row">
          <span class="keybind-label">{t(action.label)}</span>
          {#if recordingGlobalId === action.id}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="keybind-combo recording"
              type="text"
              readonly
              value={t("Нажмите комбинацию… (Esc — отмена)")}
              onkeydown={onGlobalCapture}
              autofocus
            />
          {:else}
            <button type="button" class="keybind-combo" onclick={() => startRecordingGlobal(action.id)}>
              {formatCombo(globalComboFor(action.id))}
            </button>
          {/if}
          {#if globalBinds[action.id] && globalBinds[action.id] !== action.default_combo}
            <button type="button" class="btn-icon" title={t("Сбросить к дефолту")} onclick={() => resetGlobalKeybind(action.id)}>↺</button>
          {/if}
        </div>
        {#if globalError?.actionId === action.id}
          <p class="hint" style="color:var(--danger, #d33);margin:0 0 4px 0;">
            {globalError.text}
          </p>
        {/if}
      {/each}
    </div>
    {#if globalFailed.length > 0}
      <!-- Not an input error but a fact about the environment: someone else already
           holds the combination. Staying silent is not an option — the hotkey
           simply will not fire, and that looks like a broken application. -->
      <p class="hint" style="color:var(--danger, #d33);">
        {t("Система не отдала эти комбинации (заняты другим приложением):")}
        {globalFailed.map(formatCombo).join(", ")}. {t("Выберите другие.")}
      </p>
    {/if}
 <p class="hint">{t("На Wayland (Hyprland, Sway) глобальные хоткеи перехватывает композитор — там их задают в его конфиге, биндом на запуск приложения с")}<code>--quick-task</code>, <code>--quick-note</code>,
      <code>--quick-clip</code>{t("или")}<code>--quick-pinned</code>.
    </p>

    <h4 class="keybind-group">{t("В приложении")}</h4>
    <div class="keybind-list">
      {#each KEYBIND_ACTIONS as action (action.id)}
        <div class="keybind-row">
          <span class="keybind-label">{t(action.label)}</span>
          {#if recordingActionId === action.id}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="keybind-combo recording"
              type="text"
              readonly
              value={t("Нажмите комбинацию… (Esc — отмена)")}
              onkeydown={onKeybindCapture}
              autofocus
            />
          {:else}
            <button type="button" class="keybind-combo" onclick={() => startRecording(action.id)}>
              {formatCombo(comboFor(keybinds, action.id))}
            </button>
          {/if}
          {#if keybinds[action.id] && keybinds[action.id] !== action.defaultCombo}
            <button type="button" class="btn-icon" title={t("Сбросить к дефолту")} onclick={() => resetKeybind(action.id)}>↺</button>
          {/if}
        </div>
        {#if keybindConflict?.actionId === action.id}
          <p class="hint" style="color:var(--danger, #d33);margin:0 0 4px 0;">
            {t("Конфликт: уже занято действием «{label}» — выберите другую комбинацию.", { label: keybindConflict.withLabel })}
          </p>
        {/if}
      {/each}
    </div>
  </section>

  <section class="card panel" class:hidden-by-search={sectionMatches[9] === false} class:hidden-by-tab={SECTION_TAB[9] !== activeTab} bind:this={sectionEls[9]}>
    <h3 class="section-title">{t("Статусы задач")}</h3>
    {#each statusStore.statuses as s (s.id)}
      <div class="rule-row">
        <input
          type="color"
          class="cat-color"
          value={s.color}
          title={t("Цвет статуса")}
          onchange={(e) => statusStore.update(s.id, { color: e.currentTarget.value })}
        />
        <input
          value={statusStore.name(s.id)}
          disabled={s.is_reserved}
          title={s.is_reserved ? t("Встроенный статус — название нельзя менять") : ""}
          onchange={(e) => {
            const name = e.currentTarget.value.trim();
            if (name && name !== s.name) statusStore.update(s.id, { name });
            else e.currentTarget.value = s.name;
          }}
        />
        {#if !s.is_reserved}
          <button class="btn-icon btn-danger" title={t("Удалить (задачи перейдут в «Todo»)")}
            onclick={() => statusStore.remove(s.id)}>✕</button>
        {:else}
          <span class="hint" style="margin:0;">{t("встроенный")}</span>
        {/if}
      </div>
    {/each}
    <div class="rule-row">
      <input type="color" class="cat-color" bind:value={newStatusColor} title={t("Цвет нового статуса")} />
      <input bind:value={newStatusName} placeholder={t("Новый статус (для канбана)")}
        onkeydown={(e) => { if (e.key === "Enter") addStatus(); }} />
      <button class="btn-sm" onclick={addStatus} disabled={!newStatusName.trim()}>{t("Добавить")}</button>
    </div>
    {#if statusStore.error}
      <p class="hint" style="color:var(--danger, #d33);">{tErr(statusStore.error)}</p>
    {/if}
 <p class="hint">{t("Изменения сохраняются сразу. Todo/В работе/Готово/Архив — встроенные (с ними связаны трекинг времени и завершение задач), их можно только перекрасить. Свои статусы удобны как промежуточные колонки канбан-доски; при удалении такого статуса задачи переходят в «Todo».")}</p>
  </section>

  <!-- Help: the content lives as data in lib/help.ts, this file only renders it.
       <details> instead of a custom accordion: collapsed text stays in the DOM,
       so the existing settings search (which reads el.textContent) finds it with
       no extra work — matching topics simply expand. -->
  <section class="card panel" class:hidden-by-search={sectionMatches[10] === false} class:hidden-by-tab={SECTION_TAB[10] !== activeTab} bind:this={sectionEls[10]}>
    <h3 class="section-title">{t("Справка")}</h3>
 <p class="hint" style="margin-top:0;">{t("Что умеет приложение. Раскройте тему, чтобы прочитать; поиск по настройкам ищет и здесь.")}</p>
    {#each HELP_TOPICS as topic (topic.id)}
      <details class="help-topic" open={helpSearchOpen}>
        <!-- Translated at render time rather than in help.ts: the help is pure data
             with no runes, and keeping it in one dictionary is simpler. -->
        <summary>{t(topic.title)}</summary>
        <dl class="help-list">
          {#each topic.items as item (item.term)}
            <dt>{t(item.term)}</dt>
            <dd>{t(item.desc)}</dd>
          {/each}
        </dl>
      </details>
    {/each}
  </section>

  <!-- Voice input. A separate section rather than a block inside the AI one because
       it does not depend on ai_provider at all: recognition always runs locally, so
       the model is needed even when the chat model is a cloud one. -->
  <section class="card panel" class:hidden-by-search={sectionMatches[11] === false} class:hidden-by-tab={SECTION_TAB[11] !== activeTab} bind:this={sectionEls[11]}>
    <h3 class="section-title">{t("Голосовой ввод")}</h3>
    <p class="hint" style="margin-top:0;">{t("Распознавание речи работает полностью на этом компьютере: запись никуда не отправляется. Нужна отдельная модель — её можно скачать здесь.")}</p>
    <p class="muted" style="font-size:12px;margin:0 0 10px 0;">{t("Модель распознавания хранится в")}<code>{whisperPath ?? "…"}</code>
    </p>
    <ModelDownloader kind="whisper" />
  </section>

  <!-- No "Save": the form writes itself. What is left is the receipt — without
       it an autosaving screen gives no sign that anything was stored. -->
  <p class="autosave-note" aria-live="polite">
    {#if saving}{t("Сохранение...")}{:else if saved}{t("Сохранено ✓")}{:else}{t("Изменения сохраняются сами")}{/if}
  </p>
</div>

<style>
  /* A ceiling plus centring, not a ceiling alone: without `margin: 0 auto` the
     form clung to the left edge and the empty half of a wide window read as a
     cut-off screen rather than as margins. 900 keeps a settings line short
     enough that its label stays next to its field. */
  .settings {
    max-width: 900px;
    margin: 0 auto;
    padding-bottom: 24px;
  }

  .settings-search {
    width: 100%;
    margin-bottom: 14px;
  }

  .hidden-by-search, .hidden-by-tab {
    display: none;
  }

  /* The look comes from the shared .seg; only the layout remains here. There
     are seven tabs and in a narrow window they do not fit on one line, so
     wrapping is mandatory — and with it align-self: flex-start, or the first
     row's segments would stretch to the height of the second. */
  .settings-tabs {
    margin-bottom: 14px;
    flex-wrap: wrap;
    align-self: flex-start;
    max-width: 100%;
  }

  /* The transparent top border that used to sit here compensated for .seg's
     single shared frame on a wrapped row. .seg--underline has no frame — only a
     bottom line — so the compensation would now add a stray gap above the
     second row of tabs. */
  .settings-tab {
    cursor: pointer;
  }

  .panel {
    padding: 14px 16px;
    margin-bottom: 12px;
  }

  .stack {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 14px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-size: 13px;
  }

  /* Replaced the .radio-row of native radios; the margin it carried lives on.
     No top margin: the "Тема" caption above supplies the separation now, and
     .sub-label already carries its own 6px below. Keeping both put 16px between
     the caption and its own control while the accent presets below use 6 — the
     caption looked attached to the wrong thing. */
  .theme-seg {
    margin-bottom: 12px;
  }

  /* The gap the caption needs from the field above it. .sub-label is used both
     here and for the accent presets, but only this one follows a form field. */
  .theme-label {
    margin-top: 12px;
  }

  .sub-label {
    font-size: 12px;
    color: var(--text-secondary);
    margin-bottom: 6px;
  }

  .preset-row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    align-items: center;
  }

  .autosave-note {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
    min-height: 18px;
  }

  .preset-select {
    position: relative;
    display: inline-block;
    max-width: 260px;
    width: 100%;
  }

  .preset-trigger {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    text-align: left;
  }

  .preset-trigger-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .preset-caret {
    color: var(--text-secondary);
    font-size: 10px;
  }

  /* Floats over the fields below instead of pushing them down: seventeen rows
     would otherwise shift the whole section on every open. */
  .preset-list {
    position: absolute;
    z-index: 20;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    margin: 0;
    padding: 4px;
    list-style: none;
    max-height: 260px;
    overflow-y: auto;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 8px 24px -12px rgba(0, 0, 0, .45);
  }

  /* Larger than the 11px dot used inline: in a list the swatch is the thing being
     scanned, not a marker beside a word. */
  .preset-select .swatch {
    width: 16px;
    height: 16px;
    margin-right: 0;
    vertical-align: 0;
    flex-shrink: 0;
  }

  .preset-option {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    padding: 5px 8px;
    border: none;
    border-radius: 4px;
    background: transparent;
    text-align: left;
  }

  .preset-option:hover {
    background: var(--bg-hover);
  }

  .preset-option.selected {
    color: var(--accent);
    font-weight: 600;
  }

  .input-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .swatch {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    display: inline-block;
    margin-right: 4px;
    vertical-align: -1px;
  }

  .color-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px 16px;
    max-width: 380px;
    margin-top: 12px;
  }

  .color-input {
    width: 34px;
    height: 26px;
    padding: 0;
    border-radius: 4px;
  }

  /* A colour that is still the default gets a dashed frame: the swatch shows a
     real colour either way, so without this there is no way to tell "this is
     what the theme gives you" from "I picked exactly this". */
  .color-input.is-default {
    border: 1px dashed var(--text-secondary);
    opacity: 0.75;
  }

  /* A saved set and its delete button share one row: the button sits on top of
     the option rather than beside it, so the option itself keeps the full width
     and stays a comfortable target. */
  .custom-option {
    display: flex;
    align-items: center;
  }

  .custom-option .preset-option {
    flex: 1;
    min-width: 0;
  }

  .option-del {
    flex-shrink: 0;
    padding: 2px 6px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary);
    font-size: 11px;
    line-height: 1;
  }

  .option-del:hover {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
  }

  .preset-name {
    width: 160px;
  }

  /* Sits inside the label, so it only appears next to a field that actually
     carries an override — an always-visible one would suggest every colour is
     set. */
  .unset-btn {
    margin-left: auto;
    padding: 0 5px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-size: 11px;
    line-height: 1;
  }

  .unset-btn:hover {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
  }

  .advanced-colors {
    margin-top: 14px;
  }

  .advanced-colors summary {
    font-size: 12px;
    color: var(--text-secondary);
    cursor: pointer;
    user-select: none;
    padding: 2px 0;
  }

  .advanced-colors summary:hover {
    color: var(--text-primary);
  }

  .advanced-colors .color-grid {
    margin-top: 10px;
  }

  .hint {
    font-size: 12px;
    color: var(--text-secondary);
    margin: 8px 0 0 0;
  }

  /* A hint that reports a problem rather than explaining a field: backups off,
     or the last copy being older than two cycles. */
  .hint-warn {
    color: var(--danger, #d9534f);
  }

  /* Help */
  .help-topic {
    border-top: 1px solid var(--border);
    padding: 8px 0;
  }

  .help-topic summary {
    cursor: pointer;
    font-weight: 600;
    font-size: 13px;
    list-style: none;
  }

  /* Our own arrow instead of the default marker, which different engines draw
     differently (the same principle as the icons in Icon.svelte). */
  .help-topic summary::marker,
  .help-topic summary::-webkit-details-marker { display: none; }

  .help-topic summary::before {
    content: "▸";
    display: inline-block;
    width: 14px;
    color: var(--text-secondary);
  }

  .help-topic[open] summary::before { content: "▾"; }

  .help-list {
    margin: 8px 0 4px 14px;
    font-size: 12px;
  }

  .help-list dt {
    font-weight: 600;
    margin-top: 8px;
  }

  .help-list dd {
    margin: 2px 0 0 0;
    color: var(--text-secondary);
    line-height: 1.45;
  }

  /* Suggestions sit apart from the rules themselves: they are not yet part of the
     settings and must not read as rows already in effect. */
  .rule-suggestions {
    margin-top: 6px;
    padding: 8px;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .suggestion-row {
    cursor: pointer;
    align-items: center;
  }

  .rule-row {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-bottom: 6px;
  }

  .rule-row input {
    flex: 1;
    min-width: 0;
  }

  /* The category picker in a rule row. The native <select> it replaced was sized
     by its widest option; Select fills its container instead, so the width is set
     here rather than letting it collapse to nothing in the flex row. */
  .rule-cat {
    flex: 0 0 150px;
    display: block;
  }

  .rule-row input.cat-color {
    flex: 0 0 34px;
    width: 34px;
    height: 26px;
    padding: 1px 2px;
    cursor: pointer;
  }

  .keybind-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* A group heading inside a section: global and local hotkeys live on one tab,
     but they are different mechanisms and must not read as one continuous
     list. */
  .keybind-group {
    margin: 12px 0 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
  }
  .keybind-group:first-of-type {
    margin-top: 0;
  }

  .keybind-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .keybind-label {
    flex: 1;
    font-size: 13px;
  }

  .keybind-combo {
    font-size: 12px;
    font-family: inherit;
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
    min-width: 120px;
    text-align: center;
    cursor: pointer;
  }

  .keybind-combo.recording {
    border-color: var(--accent);
    color: var(--text-secondary);
    cursor: default;
    min-width: 220px;
  }

  .key-ok {
    font-size: 11px;
    color: var(--success);
    margin-left: 6px;
    text-transform: none;
    letter-spacing: 0;
  }

  .key-warn {
    font-size: 11px;
    color: var(--cat-home);
    margin-left: 6px;
    text-transform: none;
    letter-spacing: 0;
  }

  code {
    background: var(--bg-secondary);
    padding: 1px 4px;
    border-radius: 4px;
    font-size: 0.95em;
  }
</style>
