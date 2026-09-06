<script lang="ts">
  import { Download, Keyboard, Lock, Mic, RefreshCw } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { locale, t } from "svelte-i18n";
  import ProgressBar from "../../components/ui/ProgressBar.svelte";
  import Spinner from "../../components/ui/Spinner.svelte";
  import StatusBanner from "../../components/ui/StatusBanner.svelte";
  import { SUPPORTED_LOCALES } from "../../i18n";
  import { createOnboardingController } from "./controller.svelte";
  import PermissionsStep from "./PermissionsStep.svelte";

  const controller = createOnboardingController();
  const app = controller.app;

  const titles = {
    permissions: "permissions.title",
    microphone: "onboarding.mic_title",
    model: "onboarding.model_title",
    shortcut: "onboarding.shortcut_title",
  } as const;

  const subtitles = {
    permissions: "permissions.subtitle",
    microphone: "onboarding.mic_subtitle",
    model: "onboarding.model_hint",
    shortcut: "onboarding.shortcut_subtitle",
  } as const;

  const continueLabel = $derived(
    controller.stepIndex === controller.steps.length - 1
      && (controller.step !== "model" || controller.modelReady)
      ? $t("onboarding.finish")
      : controller.step === "model" && !controller.modelReady
        ? $t("onboarding.start_button")
        : $t("onboarding.continue"),
  );

  function onContinue() {
    if (controller.step === "model" && !controller.modelReady) {
      void controller.beginDownload();
      return;
    }
    void controller.goNext();
  }

  onMount(() => {
    void controller.mount();
  });
</script>

<svelte:window onkeydown={(event) => controller.handleKeyDown(event)} />

<div class="flex h-screen items-center justify-center p-8">
  <div class="surface-card flex w-full max-w-lg flex-col gap-6 p-8">
    <div class="flex flex-col items-center gap-3 text-center">
      <img src="/favicon.svg" alt="" class="h-16 w-16 rounded-2xl" aria-hidden="true" />
      <h1 class="font-heading text-2xl font-bold">Soufflé</h1>
      <p class="text-text-secondary text-sm">{$t("onboarding.tagline")}</p>
      <p class="inline-flex items-center gap-1.5 text-xs text-text-muted">
        <Lock size={12} aria-hidden="true" />
        {$t("onboarding.local_note")}
      </p>
      <div class="flex gap-1" role="group" aria-label={$t("settings_interface.language")}>
        {#each SUPPORTED_LOCALES as loc}
          <button
            type="button"
            class={`btn px-2.5 py-1 text-xs ${$locale === loc.id ? "btn-active" : ""}`}
            onclick={() => void controller.onLocaleChange(loc.id)}
          >
            {loc.label}
          </button>
        {/each}
      </div>
    </div>

    {#if controller.steps.length > 1}
      <div
        class="flex justify-center gap-1.5"
        role="progressbar"
        aria-valuemin={1}
        aria-valuemax={controller.steps.length}
        aria-valuenow={controller.stepIndex + 1}
        aria-label={$t("onboarding.step_progress", {
          values: { current: controller.stepIndex + 1, total: controller.steps.length },
        })}
      >
        {#each controller.steps as _, i}
          <span
            class={`h-1.5 w-6 rounded-full ${i === controller.stepIndex ? "bg-accent" : "bg-surface-3"}`}
          ></span>
        {/each}
      </div>
    {/if}

    <div class="flex flex-col gap-1 text-left">
      <h2 class="font-heading text-lg font-semibold">{$t(titles[controller.step])}</h2>
      <p class="text-sm text-text-muted">{$t(subtitles[controller.step])}</p>
    </div>

    {#if controller.statusMessage}
      <StatusBanner message={controller.statusMessage} variant="warning" />
    {/if}

    {#if controller.step === "permissions"}
      <PermissionsStep onStatusChange={(status) => controller.onPermissionsStatusChange(status)} />
    {:else if controller.step === "microphone"}
      <div class="flex flex-col gap-3 text-left">
        <div class="flex items-center gap-2">
          <Mic size={16} class="shrink-0 text-text-muted" aria-hidden="true" />
          <select
            id="onboarding-mic"
            class="field-select min-w-0 flex-1"
            value={controller.selectedDevice}
            onchange={(event) => void controller.onDeviceChange(event)}
          >
            <option value="">{$t("settings_audio.input_device_automatic")}</option>
            {#each controller.audioDevices as device}
              <option value={device.uid}>
                {device.name}{device.is_default ? ` ${$t("settings_audio.device_default_suffix")}` : ""}
              </option>
            {/each}
          </select>
          <button
            type="button"
            class="btn btn-icon"
            aria-label={$t("settings_audio.refresh_devices")}
            onclick={() => void controller.refreshDevices()}
          >
            <RefreshCw size={16} />
          </button>
        </div>
        <p class="text-xs text-text-muted">{$t("onboarding.mic_hint")}</p>
      </div>
    {:else if controller.step === "model"}
      {#if controller.isDownloading}
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
      {:else if controller.isLoading}
        <div class="flex items-center justify-center gap-2 text-sm text-text-secondary">
          <Spinner />
          {$t("onboarding.loading")}
        </div>
      {:else if controller.modelReady}
        <p class="text-sm text-text-secondary">{$t("onboarding.model_ready")}</p>
      {:else}
        <div class="flex flex-col gap-2 text-left">
          <label for="onboarding-model" class="field-label">{$t("onboarding.model_label")}</label>
          <select
            id="onboarding-model"
            class="field-select"
            bind:value={controller.selectedKey}
            disabled={controller.busy}
          >
            {#each controller.options as option}
              <option value={option.key}>{option.label}</option>
            {/each}
          </select>
        </div>
      {/if}
    {:else}
      <div class="flex flex-col gap-4 text-left">
        <div class="flex items-center gap-3">
          <Keyboard size={16} class="shrink-0 text-text-muted" aria-hidden="true" />
          <button
            type="button"
            class="shortcut-button flex-1"
            class:is-recording={controller.recordingShortcut}
            onclick={() => controller.startShortcutRecording()}
          >
            {controller.recordingShortcut
              ? $t("settings_interface.press_keys")
              : controller.formatShortcut(controller.toggleShortcut) || $t("onboarding.shortcut_unset")}
          </button>
        </div>
        <p class="text-xs text-text-muted">{$t("onboarding.shortcut_hint")}</p>
        {#if controller.shortcutError}
          <StatusBanner message={$t("onboarding.shortcut_needs_modifier")} variant="danger" />
        {/if}
        <label class="flex items-start justify-between gap-4">
          <span class="flex min-w-0 flex-1 flex-col gap-0.5">
            <span class="setting-label">{$t("settings_interface.auto_paste")}</span>
            <span class="setting-desc">{$t("onboarding.auto_paste_desc")}</span>
          </span>
          <input
            type="checkbox"
            class="switch"
            checked={controller.autoPaste}
            onchange={(event) => {
              controller.autoPaste = (event.currentTarget as HTMLInputElement).checked;
            }}
            aria-label={$t("settings_interface.auto_paste")}
          />
        </label>
        {#if controller.accessibility !== "granted"}
          <p
            class="rounded-lg border border-warning/30 bg-warning/10 px-3 py-2 text-sm text-warning"
            role="status"
            data-testid="auto-paste-accessibility-warning"
          >
            {$t("onboarding.auto_paste_accessibility_warning")}
          </p>
          <div class="flex justify-end">
            <button
              type="button"
              class="btn btn-sm"
              data-testid="review-accessibility"
              onclick={() => void controller.reviewAccessibility()}
            >
              {$t("permissions.review")}
            </button>
          </div>
        {/if}
      </div>
    {/if}

    <div class="flex items-center gap-3">
      {#if controller.stepIndex > 0}
        <button
          type="button"
          class="btn btn-ghost"
          disabled={controller.busy}
          onclick={() => controller.goBack()}
        >
          {$t("onboarding.back")}
        </button>
      {:else if controller.step === "permissions"}
        <span class="text-xs text-text-muted">{$t("permissions.skip_hint")}</span>
      {/if}

      <button
        type="button"
        class="btn btn-primary ml-auto justify-center gap-2"
        disabled={controller.busy || (controller.step === "model" && !controller.modelReady && !controller.selectedKey)}
        onclick={onContinue}
      >
        {#if controller.step === "model" && !controller.modelReady && !controller.isDownloading && !controller.isLoading}
          <Download size={16} aria-hidden="true" />
        {/if}
        {#if controller.busy && controller.step === "model"}
          <Spinner />
        {/if}
        {continueLabel}
      </button>
    </div>
  </div>
</div>
