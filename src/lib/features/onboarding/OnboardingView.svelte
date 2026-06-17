<script lang="ts">
  import { onMount } from "svelte";
  import { Download, Lock } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import ProgressBar from "../../components/ui/ProgressBar.svelte";
  import Spinner from "../../components/ui/Spinner.svelte";
  import StatusBanner from "../../components/ui/StatusBanner.svelte";
  import { createOnboardingController } from "./controller.svelte";

  const controller = createOnboardingController();
  const app = controller.app;

  const isDownloading = $derived(app.transcriptionModelOperationState === "downloading");
  const isLoading = $derived(app.transcriptionModelOperationState === "loading");
  const busy = $derived(controller.isStarting || isDownloading || isLoading);

  // The model is ready: onboarding is done.
  $effect(() => {
    if (app.transcriptionRuntimePhase === "ready") {
      app.showOnboarding = false;
    }
  });

  onMount(() => {
    void controller.mount();
  });
</script>

<div class="flex h-screen items-center justify-center p-8">
  <div class="surface-card flex w-full max-w-md flex-col gap-6 p-8 text-center">
    <div class="flex flex-col items-center gap-3">
      <img src="/favicon.svg" alt="" class="h-16 w-16 rounded-2xl" aria-hidden="true" />
      <h1 class="font-heading text-2xl font-bold">Soufflé</h1>
      <p class="text-text-secondary text-sm">{$t("onboarding.tagline")}</p>
      <p class="inline-flex items-center gap-1.5 text-xs text-text-muted">
        <Lock size={12} aria-hidden="true" />
        {$t("onboarding.local_note")}
      </p>
    </div>

    {#if controller.statusMessage}
      <StatusBanner message={controller.statusMessage} variant="warning" />
    {/if}

    {#if isDownloading}
      <div class="flex flex-col gap-2 text-left">
        <p class="text-sm text-text-secondary">{$t("onboarding.downloading")}</p>
        <ProgressBar
          value={app.downloadedBytes}
          max={app.downloadTotalBytes ?? Math.max(app.downloadedBytes, 1)}
          label={$t("onboarding.downloading")}
        />
        {#if app.downloadFile}
          <p class="truncate text-xs text-text-muted">{app.downloadFile}</p>
        {/if}
      </div>
    {:else if isLoading}
      <div class="flex items-center justify-center gap-2 text-sm text-text-secondary">
        <Spinner />
        {$t("onboarding.loading")}
      </div>
    {:else}
      <div class="flex flex-col gap-2 text-left">
        <label for="onboarding-model" class="field-label">{$t("onboarding.model_label")}</label>
        <select
          id="onboarding-model"
          class="field-select"
          bind:value={controller.selectedKey}
          disabled={busy}
        >
          {#each controller.options as option}
            <option value={option.key}>{option.label}</option>
          {/each}
        </select>
        <p class="text-xs text-text-muted">{$t("onboarding.model_hint")}</p>
      </div>

      <button
        class="btn btn-primary justify-center gap-2"
        disabled={busy || !controller.selectedKey}
        onclick={() => void controller.begin()}
      >
        <Download size={16} aria-hidden="true" />
        {$t("onboarding.start_button")}
      </button>
    {/if}
  </div>
</div>
