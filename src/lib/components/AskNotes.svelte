<script lang="ts">
  // Ask-your-notes: one question, one answer, always with the notes it came from.
  //
  // A component of its own rather than a section of Notes.svelte, which is 1886
  // lines already, and rather than a panel inside SearchOverlay, which stays a
  // list of results. The entry point is a palette command: the user is already
  // typing there when the thought "where did I write about..." arrives.
  //
  // Deliberately not a conversation. Every other AI feature here is one call and
  // one answer, and the default local model is a 0.5B — a multi-turn thread is
  // where it starts losing the thread and answering the previous question.
  import { api } from "../api/tauri";
  import { t, tErr } from "../i18n.svelte";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  let { initialQuestion = "", onClose, onSelectNote }: {
    initialQuestion?: string;
    onClose: () => void;
    onSelectNote: (id: string) => void;
  } = $props();

  type Source = { id: string; title: string };

  let question = $state(initialQuestion);
  let asking = $state(false);
  let answer: string | null = $state(null);
  let sources: Source[] = $state([]);
  let errorMsg: string | null = $state(null);
  let inputEl: HTMLTextAreaElement | undefined = $state();

  // Replies are matched by request id: a slow answer to a question the user has
  // already replaced must not overwrite the new one.
  let requestId: string | null = null;

  async function ask() {
    const q = question.trim();
    if (!q || asking) return;
    const id = crypto.randomUUID();
    requestId = id;
    asking = true;
    answer = null;
    sources = [];
    errorMsg = null;
    try {
      await api.aiAskNotes(id, q);
    } catch (e) {
      if (requestId !== id) return;
      asking = false;
      errorMsg = String(e);
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
    // Enter asks, Shift+Enter breaks the line — the same rule as the quick
    // capture window, so the gesture carries over.
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); ask(); }
  }

  onMount(() => {
    inputEl?.focus();
    let un: (() => void) | undefined;
    (async () => {
      un = await listen<{
        request_id: string;
        result: string | null;
        sources: Source[];
        error: string | null;
      }>("ai-ask-notes", (e) => {
        if (e.payload.request_id !== requestId) return;
        asking = false;
        answer = e.payload.result;
        sources = e.payload.sources ?? [];
        errorMsg = e.payload.error;
      });
    })();
    if (initialQuestion.trim()) ask();
    return () => un?.();
  });
</script>

<svelte:window onkeydown={onKeydown} />

<div
  role="button"
  tabindex="-1"
  class="overlay backdrop"
  onclick={(e) => { if (e.target === e.currentTarget) onClose(); }}
  onkeydown={() => {}}
>
  <div class="modal panel ask-modal">
    <div class="ask-head">
      <h3>{t("Спросить заметки")}</h3>
    </div>

    <textarea
      class="ask-input"
      bind:this={inputEl}
      bind:value={question}
      rows="2"
      placeholder={t("О чём спросить? (Enter — спросить)")}
    ></textarea>

    <div class="ask-actions">
      <button class="btn btn-primary" onclick={ask} disabled={asking || !question.trim()}>
        {asking ? t("Ищу...") : t("Спросить")}
      </button>
    </div>

    {#if errorMsg}
      <p class="ask-error">{tErr(errorMsg)}</p>
    {:else if answer !== null}
      <div class="ask-answer">{answer}</div>

      {#if sources.length > 0}
        <!-- Always shown, never optional: an answer the user cannot trace back
             to its notes is one they have to take on faith, and a small local
             model is not worth that faith. -->
        <div class="ask-sources">
          <div class="ask-sources-title">{t("По заметкам")}</div>
          {#each sources as s (s.id)}
            <button class="ask-source" onclick={() => onSelectNote(s.id)}>{s.title}</button>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</div>

<style>
  .ask-modal {
    max-width: 560px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .ask-head h3 {
    margin: 0;
    font-size: 15px;
  }
  .ask-input {
    width: 100%;
    resize: vertical;
    font: inherit;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-secondary);
    color: var(--text-primary);
  }
  .ask-actions {
    display: flex;
    justify-content: flex-end;
  }
  .ask-answer {
    white-space: pre-wrap;
    line-height: 1.5;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--bg-card);
    border: 1px solid var(--border);
  }
  .ask-error {
    margin: 0;
    color: var(--danger);
    font-size: 13px;
  }
  .ask-sources {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .ask-sources-title {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-secondary);
    width: 100%;
  }
  .ask-source {
    font: inherit;
    font-size: 12px;
    padding: 3px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
  }
  .ask-source:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
