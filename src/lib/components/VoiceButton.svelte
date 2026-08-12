<script lang="ts">
  // The microphone button. Records while pressed-in, transcribes on the second
  // click, and hands the recognised text to the caller — where it goes differs
  // per call site (CodeMirror wants a dispatch, a textarea wants its value), so
  // this component deliberately does not insert anything itself.
  //
  // The button is absent, not disabled, when voice input is unavailable: the same
  // capability detection the project uses for window tracking and notification
  // actions. A disabled button would raise a question the user cannot answer from
  // where they are standing.
  import { onMount } from "svelte";
  import { voice } from "../voice.svelte";
  import { t } from "../i18n.svelte";

  let { onText, title }: { onText: (text: string) => void; title?: string } = $props();

  // The state is shared with the Ctrl+Shift+D hotkey (lib/voice.svelte.ts): both
  // drive one recording, so pressing the key and clicking the button are the same
  // action rather than two that can disagree.
  const available = $derived(voice.available);
  const recording = $derived(voice.recording);
  const busy = $derived(voice.busy);
  const error = $derived(voice.error);

  onMount(() => { voice.ensureChecked(); });

  async function toggle() {
    const text = await voice.toggle();
    if (text) onText(text);
  }
</script>

{#if available}
  <button
    class="voice-btn"
    class:recording
    onclick={toggle}
    disabled={busy && !recording}
    title={error ?? title ?? (recording ? t("Остановить и распознать (Ctrl+Shift+D)") : t("Надиктовать (Ctrl+Shift+D)"))}
    aria-label={recording ? t("Остановить и распознать") : t("Надиктовать")}
  >
    {#if busy && !recording}
      <span class="spinner"></span>
    {:else}
      <!-- A filled circle while recording, a microphone otherwise: the state has to
           be readable without hovering for the tooltip. -->
      <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
        {#if recording}
          <circle cx="12" cy="12" r="7" fill="currentColor" />
        {:else}
          <path
            d="M12 15a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3z M19 11a7 7 0 0 1-14 0 M12 18v3"
            fill="none" stroke="currentColor" stroke-width="1.8"
            stroke-linecap="round" stroke-linejoin="round"
          />
        {/if}
      </svg>
    {/if}
  </button>
{/if}

<style>
  .voice-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-primary);
    color: var(--text-secondary);
    cursor: pointer;
  }
  .voice-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--accent);
  }
  .voice-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }
  /* Recording is the one state that must be noticeable from across the window. */
  .voice-btn.recording {
    color: var(--danger);
    border-color: var(--danger);
    animation: voice-pulse 1.4s ease-in-out infinite;
  }
  @keyframes voice-pulse {
    50% { opacity: 0.45; }
  }
  .spinner {
    width: 12px;
    height: 12px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: voice-spin 0.7s linear infinite;
  }
  @keyframes voice-spin {
    to { transform: rotate(360deg); }
  }

  /* Users who ask the OS for less motion get a static indicator: the colour and
     the shape already carry the state. */
  @media (prefers-reduced-motion: reduce) {
    .voice-btn.recording { animation: none; }
    .spinner { animation-duration: 2s; }
  }
</style>
