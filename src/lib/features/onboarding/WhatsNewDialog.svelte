<script lang="ts">
  import { t } from "svelte-i18n";

  let {
    version,
    releaseNotes,
    onDismiss,
  }: {
    version: string;
    releaseNotes: string;
    onDismiss: () => void;
  } = $props();
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

    <div class="min-h-0 flex-1 overflow-y-auto rounded-lg bg-surface-1/70 p-4 text-sm leading-relaxed text-text-secondary whitespace-pre-wrap">
      {releaseNotes}
    </div>

    <div class="flex justify-end">
      <button onclick={onDismiss} class="btn btn-active">
        {$t("whats_new.got_it")}
      </button>
    </div>
  </div>
</div>
