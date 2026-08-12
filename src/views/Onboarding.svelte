<script lang="ts">
  import { enable as enableAutostart, disable as disableAutostart } from "@tauri-apps/plugin-autostart";
  import { api } from "../lib/api/tauri";
  import type { AppSettings } from "../lib/types";
  import ModelDownloader from "../lib/components/ModelDownloader.svelte";

  import { t, i18n } from "../lib/i18n.svelte";
  import type { Lang } from "../lib/i18n";
  interface Props {
    settings: AppSettings;
    isWayland: boolean;
    onDone: () => void;
  }
  let { settings, isWayland, onDone }: Props = $props();

  // Step 0 (language) comes first for a reason: every screen after it is text, so
  // choosing the language later would mean reading the whole onboarding in the
  // wrong one. Step 3 (Wayland monitoring) and step 7 (compositor binds) are shown
  // on Wayland only. Step 6 (voice input) sits before the closing one and is
  // entirely optional: nothing here has to be downloaded for the app to work, the
  // step just makes the feature discoverable at all.
  const steps = isWayland ? [0, 1, 2, 3, 4, 7, 6, 5] : [0, 1, 2, 4, 6, 5];
  let stepIdx = $state(0);
  let step = $derived(steps[stepIdx]);

  // Seeded from the language already in effect (i18n.init detected it from the
  // system locale), so the step confirms a sensible default rather than starting
  // blank. Applied immediately on click — the point of choosing it first is that
  // the rest of the onboarding is already in that language.
  let lang = $state<Lang>(i18n.lang);
  function chooseLang(code: Lang) {
    lang = code;
    i18n.set(code);
  }

  // The compositor snippets. The four quick-capture actions and their default
  // combinations are the backend's (GLOBAL_ACTIONS in commands/hotkeys.rs), and
  // the CLI flags are the ones quick_mode_from_args parses — these two lists have
  // to agree with it, which is what the accompanying test checks.
  const QUICK_BINDS: { mods: string; key: string; flag: string }[] = [
    { mods: "CTRL SHIFT", key: "N", flag: "--quick-task" },
    { mods: "CTRL SHIFT", key: "M", flag: "--quick-note" },
    { mods: "CTRL SHIFT", key: "B", flag: "--quick-clip" },
    { mods: "CTRL SHIFT", key: "J", flag: "--quick-pinned" },
  ];

  const hyprConf = QUICK_BINDS
    .map(b => `bind = ${b.mods}, ${b.key}, exec, kriptag ${b.flag}`)
    .join("\n");

  // Sway spells the modifiers differently and takes the whole combination as one
  // token, so the same data cannot be printed with one template.
  const swayConf = QUICK_BINDS
    .map(b => `bindsym ${b.mods.split(" ").map(m => m === "CTRL" ? "Control" : "Shift").join("+")}+${b.key.toLowerCase()} exec kriptag ${b.flag}`)
    .join("\n");

  let copied = $state<string | null>(null);
  async function copyConf(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = text;
      setTimeout(() => { if (copied === text) copied = null; }, 2000);
    } catch {
      // Clipboard access can be refused; the text is on screen and selectable,
      // so there is nothing to report — failing silently beats an error the user
      // can do nothing about.
    }
  }

  let aiChoice = $state<"local" | "cloud" | "none">("none");
  let autostart = $state(false);
  let error: string | null = $state(null);
  let finishing = $state(false);

  function next() {
    if (stepIdx < steps.length - 1) stepIdx += 1;
  }
  function back() {
    if (stepIdx > 0) stepIdx -= 1;
  }

  async function finish() {
    finishing = true;
    error = null;
    try {
      if (autostart) {
        await enableAutostart();
      } else {
        await disableAutostart().catch(() => {});
      }
      settings.language = lang;
      settings.ai_provider =
        aiChoice === "cloud" ? "openai" : aiChoice === "none" ? "none" : "local";
      settings.onboarding_complete = true;
      await api.saveSettings(settings);
      onDone();
    } catch (e) {
      error = String(e);
    } finally {
      finishing = false;
    }
  }
</script>

<div class="wrap">
  <div class="card box">
    <div class="progress">
      <span class="muted" style="font-size:12px;">{t("Шаг {i} из {n}", { i: stepIdx + 1, n: steps.length })}</span>
      <div class="steps-track">
        {#each steps as _, i}
          <span class="step-dot" class:done={i <= stepIdx}></span>
        {/each}
      </div>
    </div>

    {#if error}
      <div class="alert">{error}</div>
    {/if}

    {#if step === 0}
      <!-- Both names are written in their own language and never translated: at
           this point the user may not read the interface language at all, so
           "Русский"/"English" are the only labels that work either way. -->
      <h2>Kriptag</h2>
      <p class="muted">{t("Выберите язык интерфейса")}</p>
      <div class="options" style="margin-top:12px;">
        <label class="option">
          <input type="radio" name="lang" value="ru" checked={lang === "ru"} onchange={() => chooseLang("ru")} />
          <!-- i18n-ok-line: a language names itself and is never translated -->
          <span><b>Русский</b></span>
        </label>
        <label class="option">
          <input type="radio" name="lang" value="en" checked={lang === "en"} onchange={() => chooseLang("en")} />
          <span><b>English</b></span>
        </label>
      </div>
      <p class="muted" style="font-size:13px;margin-top:10px;">{t("Язык можно сменить позже в Настройках.")}</p>
    {:else if step === 1}
      <h2>{t("Добро пожаловать в Kriptag")}</h2>
      <p>{t("Задачи, заметки и мониторинг активности — всё локально, приватно и с опциональным ИИ.")}</p>
      <p class="muted">{t("Пара минут настройки — и можно работать.")}</p>
    {:else if step === 2}
      <h2>{t("ИИ-помощник")}</h2>
      <p>{t("ИИ переписывает задачи в SMART-формат, генерирует подзадачи и классифицирует их.")}</p>
      <div class="options">
        <label class="option">
          <input type="radio" name="ai" value="local" bind:group={aiChoice} />
          <span><b>{t("Локальная модель")}</b><br/>
            <small class="muted">{t("Приватно, работает оффлайн. GGUF-модель можно скачать прямо здесь.")}</small></span>
        </label>
        {#if aiChoice === "local"}
          <div style="margin:4px 0 4px 26px;">
            <ModelDownloader />
          </div>
        {/if}
        <label class="option">
          <input type="radio" name="ai" value="cloud" bind:group={aiChoice} />
          <span><b>{t("Облачный API")}</b><br/>
            <small class="muted">{t("OpenAI или Anthropic — API-ключ вводится в Настройках")}</small></span>
        </label>
        <label class="option">
          <input type="radio" name="ai" value="none" bind:group={aiChoice} />
          <span><b>{t("Без ИИ")}</b><br/>
            <small class="muted">{t("Можно включить позже в Настройках")}</small></span>
        </label>
      </div>
    {:else if step === 3}
      <h2>{t("Мониторинг на Wayland")}</h2>
      <p>
        {t("Активность отслеживается системно: композитор сам сообщает о простое и возврате (протокол")} <code>ext-idle-notify</code>{t("). Настраивать ничего не нужно, содержимое ввода приложению не видно — только факт активности.")}
      </p>
      <p class="muted" style="font-size:13px;">
        {t("Если композитор не поддерживает протокол, трекинг работает только при окне в фокусе. Текущий режим виден в Настройках → Мониторинг.")}
      </p>
    {:else if step === 4}
      <h2>{t("Автозагрузка и хоткеи")}</h2>
      <label class="option" style="margin-bottom:12px;align-items:center;">
        <input type="checkbox" bind:checked={autostart} />
        {t("Запускать Kriptag при входе в систему")}
      </label>
      <p>{t("Быстрый ввод из любого места, не открывая окно:")}</p>
      <ul class="keylist">
        <li><kbd>Ctrl Shift N</kbd> {t("— задача")}</li>
        <li><kbd>Ctrl Shift M</kbd> {t("— заметка")}</li>
        <li><kbd>Ctrl Shift B</kbd> {t("— заметка из буфера обмена")}</li>
        <li><kbd>Ctrl Shift J</kbd> {t("— быстрый слот")}</li>
      </ul>
      <!-- On Wayland the compositor owns these combinations, and saying so here
           would only half-explain it — the next step is the config itself. -->
      {#if isWayland}
        <p class="muted" style="font-size:13px;">
          {t("На Wayland их регистрирует композитор — как это прописать, на следующем шаге.")}
        </p>
      {:else}
        <p class="muted" style="font-size:13px;">
          {t("Комбинации можно изменить в Настройках → Хоткеи.")}
        </p>
      {/if}
    {:else if step === 7}
      <h2>{t("Хоткеи в композиторе")}</h2>
      <p>
        {t("На Wayland глобальные хоткеи регистрирует композитор, а не приложение — поэтому их нужно прописать в его конфиге. Скопируйте строки под свой композитор:")}
      </p>
      <div class="conf-block">
        <div class="conf-head">
          <span class="muted">Hyprland — <code>~/.config/hypr/hyprland.conf</code></span>
          <button class="btn-sm" onclick={() => copyConf(hyprConf)}>{copied === hyprConf ? t("Скопировано ✓") : t("Копировать")}</button>
        </div>
        <pre>{hyprConf}</pre>
      </div>
      <div class="conf-block">
        <div class="conf-head">
          <span class="muted">Sway — <code>~/.config/sway/config</code></span>
          <button class="btn-sm" onclick={() => copyConf(swayConf)}>{copied === swayConf ? t("Скопировано ✓") : t("Копировать")}</button>
        </div>
        <pre>{swayConf}</pre>
      </div>
      <p class="muted" style="font-size:13px;">
        {t("Команда — это имя бинарника; если Kriptag не установлен в систему, укажите полный путь до него. Комбинации можно изменить в Настройках → Хоткеи.")}
      </p>
    {:else if step === 6}
      <h2>{t("Голосовой ввод")}</h2>
      <p>{t("Заметки и быстрый ввод можно надиктовывать. Речь распознаётся на этом компьютере — запись никуда не отправляется.")}</p>
      <p class="muted" style="font-size:13px;">{t("Нужна отдельная модель распознавания. Шаг можно пропустить — модель ставится позже в Настройках → ИИ.")}</p>
      <div style="margin-top:10px;">
        <ModelDownloader kind="whisper" />
      </div>
    {:else}
      <h2>{t("Готово!")}</h2>
      <ul>
        <li><b>{t("Сегодня")}</b> {t("— экран «что сейчас»:")} <kbd>Ctrl `</kbd></li>
        <li><b>{t("Задачи")}</b> {t("— список и доска, переключение")} <kbd>Ctrl Tab</kbd></li>
        <li><b>{t("Заметки")}</b> {t("— Markdown, вики-ссылки [[…]] и граф связей")}</li>
        <li><b>{t("Трей")}</b> {t("— режимы Focus (без уведомлений) и Study (помодоро)")}</li>
      </ul>
      <!-- The onboarding deliberately stays short: the full tour of the features
           lives in one place, Settings -> Help. Duplicating it here would mean
           two texts that diverge at the first edit. -->
      <p class="muted" style="font-size:13px;margin-top:10px;">
        {t("Остальное — в")} <b>{t("Настройках → Справка")}</b>{t(": там собрано, что умеют заметки, задачи, быстрый ввод, ИИ и мониторинг.")}
      </p>
    {/if}

    <div class="actions">
      {#if stepIdx > 0}
        <button class="btn-ghost" onclick={back}>{t("Назад")}</button>
      {/if}
      <span style="flex:1;"></span>
      {#if stepIdx < steps.length - 1}
        <button class="btn-primary" onclick={next}>{step === 1 ? t("Начать настройку") : t("Далее")}</button>
      {:else}
        <button class="btn-primary" onclick={finish} disabled={finishing}>{finishing ? t("Сохранение...") : t("Начать")}</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .wrap {
    height: 100vh;
    overflow-y: auto;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 48px 16px;
  }

  .box {
    width: 100%;
    max-width: 480px;
    padding: 22px 24px;
  }

  .progress {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
  }

  .conf-block {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    margin: 10px 0;
  }

  .conf-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .conf-head span { flex: 1; min-width: 0; }

  .conf-block pre {
    margin: 0;
    padding: 8px;
    font-size: 12px;
    line-height: 1.6;
    /* The lines are long and must not wrap: a wrapped bind reads as two binds,
       and copying it by hand would produce a broken config. */
    overflow-x: auto;
    white-space: pre;
  }

  .keylist {
    margin: 6px 0 0;
    padding-left: 18px;
    line-height: 1.9;
  }

  .steps-track {
    display: flex;
    gap: 4px;
  }

  .step-dot {
    width: 18px;
    height: 4px;
    border-radius: 2px;
    background: var(--bg-hover);
  }

  .step-dot.done {
    background: var(--accent);
  }

  h2 {
    margin: 0 0 10px 0;
    font-size: 17px;
  }

  p { margin: 0 0 10px 0; font-size: 13px; }

  ul {
    font-size: 13px;
    padding-left: 18px;
    margin: 0;
  }

  ul li { margin: 4px 0; }

  .options {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .option {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    cursor: pointer;
    font-size: 13px;
  }

  pre {
    background: var(--bg-secondary);
    padding: 8px 12px;
    border-radius: var(--radius);
    font-size: 12px;
    overflow-x: auto;
  }

  code {
    background: var(--bg-secondary);
    padding: 1px 4px;
    border-radius: 4px;
    font-size: 0.95em;
  }

  .actions {
    display: flex;
    gap: 8px;
    margin-top: 20px;
  }
</style>
