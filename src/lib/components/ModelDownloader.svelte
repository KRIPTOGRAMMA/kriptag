<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api } from "../api/tauri";
  import type { ModelOption, ModelKind } from "../types";

  import { t } from "../i18n.svelte";

  // Which model this picker manages. Defaults to the chat model: the component
  // existed for it alone before voice input, and both call sites that predate the
  // prop mean exactly that.
  let { kind = "llm" as ModelKind }: { kind?: ModelKind } = $props();

  let options: ModelOption[] = $state([]);
  let selectedId: string = $state("");
  let customUrl = $state("");
  let usingCustomUrl = $state(false);
  let exists = $state(false);
  let installedUrl: string | null = $state(null);
  let sizeBytes = $state(0);
  let downloading = $state(false);
  let pct = $state(0);
  let error: string | null = $state(null);
  let unlisten: (() => void) | null = null;

  const mb = (b: number) => (b / 1024 / 1024).toFixed(1);
  const gb = (b: number) => (b / 1024 / 1024 / 1024).toFixed(1);

  const selectedUrl = $derived(
    usingCustomUrl ? customUrl : (options.find(o => o.id === selectedId)?.url ?? "")
  );

  async function refresh() {
    const s = await api.modelStatus(kind);
    exists = s.exists;
    sizeBytes = s.size_bytes;
    installedUrl = s.installed_url;
  }

  /// Points the picker at the model that is actually installed.
  ///
  /// Without this the picker fell back to the recommended entry every time
  /// Settings was reopened, so it claimed an install the user never made — the
  /// file on disk has a fixed name and carries no identity, which is why the url
  /// is remembered in settings instead.
  ///
  /// A url outside the catalogue means the user pasted their own, so the custom
  /// field is reopened with it rather than silently losing it.
  function selectInstalled() {
    if (!installedUrl) return;
    const match = options.find(o => o.url === installedUrl);
    if (match) {
      selectedId = match.id;
      usingCustomUrl = false;
    } else {
      usingCustomUrl = true;
      customUrl = installedUrl;
    }
  }

  onMount(async () => {
    try {
      options = await api.listModelOptions(kind);
      const recommended = options.find(o => o.recommended) ?? options[0];
      if (recommended) selectedId = recommended.id;
      await refresh();
      // After refresh, so the installed model wins over the recommended default.
      // The recommendation is only what to pick when nothing is installed yet.
      selectInstalled();
      // Both pickers hear the same event, so each ignores the other's progress —
      // otherwise downloading the chat model would animate the voice picker too.
      unlisten = await listen<{ pct: number; kind: ModelKind }>("model-download-progress", ({ payload }) => {
        if (payload.kind !== kind) return;
        pct = payload.pct;
      });
    } catch (e) {
      error = String(e);
    }
  });

  onDestroy(() => unlisten?.());

  async function download() {
    if (!selectedUrl) return;
    error = null;
    downloading = true;
    pct = 0;
    try {
      await api.downloadModel(selectedUrl, kind);
      await refresh();
    } catch (e) {
      error = String(e);
    } finally {
      downloading = false;
    }
  }
</script>

<div class="model-picker">
  {#if exists}
    <div class="status ok">✓ {t("Модель загружена ({mb} МБ)", { mb: mb(sizeBytes) })}</div>
  {:else}
    <div class="status">{t("Модель не найдена")}</div>
  {/if}

  <div class="option-list">
    {#each options as opt (opt.id)}
      <label class="option" class:active={!usingCustomUrl && selectedId === opt.id}>
        <input
          type="radio"
          name="model-option-{kind}"
          checked={!usingCustomUrl && selectedId === opt.id}
          disabled={downloading}
          onchange={() => { usingCustomUrl = false; selectedId = opt.id; }}
        />
        <div class="option-body">
          <div class="option-title">
            {opt.name}
            {#if opt.recommended}<span class="chip-recommended">{t("рекомендуется")}</span>{/if}
          </div>
          <div class="option-meta">{t("~{gb} ГБ · от {ram} ГБ ОЗУ", { gb: gb(opt.size_bytes), ram: opt.ram_gb })}</div>
          <!--
            The description arrives from Rust (commands/model.rs) as a fixed
            string rather than from the DB, so it is translated at render time,
            like the help and the hotkey names. Coverage is verified by the
            "model descriptions (model.rs) are in the EN dictionary" test.
          -->
          <div class="option-desc">{t(opt.description)}</div>
        </div>
      </label>
    {/each}

    <label class="option" class:active={usingCustomUrl}>
      <input
        type="radio"
        name="model-option-{kind}"
        checked={usingCustomUrl}
        disabled={downloading}
        onchange={() => { usingCustomUrl = true; }}
      />
      <div class="option-body">
        <div class="option-title">{kind === "whisper" ? t("Свой URL (ggml)") : t("Свой URL (GGUF)")}</div>
        <input
          type="text"
          bind:value={customUrl}
          disabled={downloading}
          placeholder={kind === "whisper" ? "https://.../ggml-base.bin" : "https://.../model.gguf"}
          class="custom-url-input"
          onfocus={() => { usingCustomUrl = true; }}
        />
      </div>
    </label>
  </div>

  {#if downloading}
    <div class="progress-track">
      <div class="progress-fill" style="width:{pct}%;"></div>
    </div>
    <div class="progress-label">{t("Загрузка… {pct}%", { pct })}</div>
  {:else}
    <button class="btn-primary btn-sm" onclick={download} disabled={!selectedUrl}>
      {exists ? t("Перекачать") : t("Скачать модель")}
    </button>
  {/if}

  {#if error}
    <div class="error">{error}</div>
  {/if}
</div>

<style>
  .model-picker {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .status {
    font-size: 13px;
    color: var(--text-secondary);
  }
  .status.ok {
    color: var(--success);
  }
  .option-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .option {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
  }
  .option.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }
  .option input[type="radio"] {
    margin-top: 3px;
    flex-shrink: 0;
  }
  .option-body {
    flex: 1;
    min-width: 0;
  }
  .option-title {
    font-size: 13px;
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .chip-recommended {
    font-size: 10px;
    font-weight: 500;
    padding: 1px 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--accent);
  }
  .option-meta {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: 2px;
  }
  .option-desc {
    font-size: 12px;
    color: var(--text-secondary);
    margin-top: 3px;
  }
  .custom-url-input {
    display: block;
    width: 100%;
    margin-top: 6px;
    box-sizing: border-box;
    font-size: 12px;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-primary);
    color: var(--text-primary);
  }
  .progress-track {
    background: var(--bg-secondary);
    border-radius: 6px;
    height: 10px;
    overflow: hidden;
  }
  .progress-fill {
    background: var(--accent);
    height: 100%;
    transition: width 0.2s;
  }
  .progress-label {
    font-size: 12px;
    color: var(--text-secondary);
  }
  .error {
    font-size: 12px;
    color: var(--danger);
  }
</style>
