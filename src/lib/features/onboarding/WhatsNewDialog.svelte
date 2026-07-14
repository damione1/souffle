<script lang="ts">
  import { t } from "svelte-i18n";
  import { openReleasePage } from "../../api/diagnostics";
  import { renderReleaseNotesMarkdown } from "../../utils";

  let {
    version,
    releaseNotes,
    onDismiss,
  }: {
    version: string;
    releaseNotes: string;
    onDismiss: () => void;
  } = $props();

  // renderReleaseNotesMarkdown escapes all input text and emits only a fixed
  // set of tags, so this HTML is safe to inject (see its module docs).
  let notesHtml = $derived(renderReleaseNotesMarkdown(releaseNotes));

  // WKWebView suppresses new-window navigation and the dialog must not
  // navigate in place, so link clicks are routed to the system browser via
  // the backend command (which only accepts https://github.com/ URLs, the
  // only kind the renderer emits).
  function handleNotesClick(event: MouseEvent) {
    const anchor = (event.target as HTMLElement).closest("a");
    if (!anchor) return;
    event.preventDefault();
    void openReleasePage(anchor.href);
  }
</script>

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6 backdrop-blur-sm"
  role="dialog"
  aria-modal="true"
  aria-label={$t("whats_new.title")}
>
  <div class="surface-card flex w-full max-w-lg max-h-[80vh] flex-col gap-4">
    <div class="flex flex-col gap-1">
      <h2 class="font-heading text-lg font-bold">{$t("whats_new.title")}</h2>
      <p class="text-sm text-text-muted">
        {$t("whats_new.subtitle", { values: { version } })}
      </p>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div
      class="release-notes min-h-0 flex-1 overflow-y-auto rounded-lg bg-surface-1/70 p-4 text-sm leading-relaxed text-text-secondary"
      onclick={handleNotesClick}
    >
      {@html notesHtml}
    </div>

    <div class="flex justify-end">
      <button onclick={onDismiss} class="btn btn-active">
        {$t("whats_new.got_it")}
      </button>
    </div>
  </div>
</div>
