<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import AboutSettingsSection from "../features/settings/components/AboutSettingsSection.svelte";
  import AudioSettingsSection from "../features/settings/components/AudioSettingsSection.svelte";
  import DiagnosticsSettingsSection from "../features/settings/components/DiagnosticsSettingsSection.svelte";
  import IntelligenceSettingsSection from "../features/settings/components/IntelligenceSettingsSection.svelte";
  import InterfaceSettingsSection from "../features/settings/components/InterfaceSettingsSection.svelte";
  import { createSettingsController } from "../features/settings/controller.svelte";
  import { formatSelectedTranscriptionLabel } from "../features/transcription/catalog";
  import ModelGateSection from "../features/transcription/components/ModelGateSection.svelte";
  import StatusBanner from "./ui/StatusBanner.svelte";

  const controller = createSettingsController();

  let selectedTranscriptionLabel = $derived(
    formatSelectedTranscriptionLabel(
      controller.catalog,
      controller.app.settings.transcription_engine_id,
      controller.app.settings.transcription_model_id,
      controller.app.settings.transcription_backend_id,
    ) || $t("settings.no_model_selected"),
  );

  let selectedOllamaModelLabel = $derived(
    controller.summaryModels.find((model) => model.id === controller.app.settings.ollama_model)?.label
    ?? controller.app.settings.ollama_model,
  );

  onMount(() => {
    void controller.mount();
  });
</script>

<svelte:window onkeydown={controller.handleKeyDown} />

<div class="flex flex-col gap-4">
  <h2>{$t("settings.title")}</h2>

  {#if controller.statusMessage}
    <StatusBanner message={controller.statusMessage} />
  {/if}

  <AudioSettingsSection
    audioDevices={controller.audioDevices}
    selectedDevice={controller.app.selectedDevice}
    onDeviceChange={controller.onDeviceChange}
    onRefreshDevices={controller.refreshDevices}
  />

  <ModelGateSection
    catalog={controller.catalog}
    selectedEngineId={controller.app.settings.transcription_engine_id}
    selectedModelId={controller.app.settings.transcription_model_id}
    selectedBackendId={controller.app.settings.transcription_backend_id}
    runtimePhase={controller.runtimePhase}
    modelOperationState={controller.modelOperationState}
    downloadFile={controller.downloadFile}
    downloadCompletedFiles={controller.downloadCompletedFiles}
    downloadTotalFiles={controller.downloadTotalFiles}
    onSelectEngine={controller.selectTranscriptionEngine}
    onSelectModel={controller.selectTranscriptionModel}
    onSelectBackend={controller.selectTranscriptionBackend}
    onDownloadModel={controller.handleDownloadModel}
    onLoadModel={controller.handleLoadModel}
  />

  <IntelligenceSettingsSection
    ollamaUrl={controller.app.settings.ollama_url}
    ollamaAvailable={controller.ollamaAvailable}
    ollamaModels={controller.ollamaModels}
    summaryModels={controller.summaryModels}
    selectedOllamaModel={controller.app.settings.ollama_model}
    onOllamaUrlChange={controller.onOllamaUrlChange}
    onOllamaModelChange={controller.onOllamaModelChange}
    onRetryOllama={controller.checkOllama}
  />

  <InterfaceSettingsSection
    theme={controller.app.settings.theme}
    locale={controller.app.settings.locale}
    autoPaste={controller.app.settings.auto_paste}
    pasteDelayMs={controller.app.settings.paste_delay_ms}
    toggleShortcut={controller.toggleShortcut}
    pttShortcut={controller.pttShortcut}
    recordingField={controller.recordingField}
    shortcutError={controller.shortcutError}
    onThemeChange={controller.onThemeChange}
    onLocaleChange={controller.onLocaleChange}
    onAutoPasteChange={controller.onAutoPasteChange}
    onPasteDelayChange={controller.onPasteDelayChange}
    onStartRecording={controller.startRecording}
    onClearShortcut={controller.clearShortcut}
    formatShortcut={controller.formatShortcut}
  />

  <DiagnosticsSettingsSection
    debugTranscription={controller.app.settings.debug_transcription}
    onDebugTranscriptionChange={controller.onDebugTranscriptionChange}
  />

  <AboutSettingsSection
    selectedTranscriptionLabel={selectedTranscriptionLabel}
    selectedOllamaModelLabel={selectedOllamaModelLabel}
  />
</div>
