<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import AboutSettingsSection from "../features/settings/components/AboutSettingsSection.svelte";
  import AdvancedSettingsSection from "../features/settings/components/AdvancedSettingsSection.svelte";
  import AudioSettingsSection from "../features/settings/components/AudioSettingsSection.svelte";
  import DictionarySettingsSection from "../features/settings/components/DictionarySettingsSection.svelte";
  import DiagnosticsSettingsSection from "../features/settings/components/DiagnosticsSettingsSection.svelte";
  import IntelligenceSettingsSection from "../features/settings/components/IntelligenceSettingsSection.svelte";
  import InterfaceSettingsSection from "../features/settings/components/InterfaceSettingsSection.svelte";
  import ModelSettingsSection from "../features/settings/components/ModelSettingsSection.svelte";
  import { createSettingsController } from "../features/settings/controller.svelte";
  import { formatSelectedTranscriptionLabel } from "../features/transcription/catalog";
  import ConfirmAction from "./ui/ConfirmAction.svelte";
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
  {#if controller.statusMessage}
    <StatusBanner message={controller.statusMessage} />
  {/if}

  <ModelSettingsSection
    catalog={controller.catalog}
    selectedEngineId={controller.app.settings.transcription_engine_id}
    selectedModelId={controller.app.settings.transcription_model_id}
    runtimePhase={controller.runtimePhase}
    operationState={controller.modelOperationState}
    downloadedBytes={controller.downloadedBytes}
    downloadTotalBytes={controller.downloadTotalBytes}
    downloadFile={controller.downloadFile}
    onSelectModel={controller.selectModelOption}
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

  <DictionarySettingsSection
    entries={controller.dictionaryEntries}
    onAdd={controller.handleAddDictionaryEntry}
    onDelete={controller.handleDeleteDictionaryEntry}
  />

  <AdvancedSettingsSection>
    <AudioSettingsSection
      audioDevices={controller.audioDevices}
      selectedDevice={controller.app.selectedDevice}
      captureSystemAudio={controller.app.settings.capture_system_audio}
      systemAudioSupported={controller.systemAudioSupported}
      vadEnabled={controller.app.settings.vad_enabled}
      fillerRemoval={controller.app.settings.filler_removal}
      stutterCollapse={controller.app.settings.stutter_collapse}
      dictionaryCorrection={controller.app.settings.dictionary_correction}
      onDeviceChange={controller.onDeviceChange}
      onRefreshDevices={controller.refreshDevices}
      onCaptureSystemAudioChange={controller.onCaptureSystemAudioChange}
      onVadEnabledChange={controller.onVadEnabledChange}
      onFillerRemovalChange={controller.onFillerRemovalChange}
      onStutterCollapseChange={controller.onStutterCollapseChange}
      onDictionaryCorrectionChange={controller.onDictionaryCorrectionChange}
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

    <DiagnosticsSettingsSection
      debugTranscription={controller.app.settings.debug_transcription}
      onDebugTranscriptionChange={controller.onDebugTranscriptionChange}
    />

    <section class="surface-card flex flex-col gap-2">
      <h3>{$t("settings_advanced.model_storage")}</h3>
      <p class="text-text-secondary text-sm">{$t("settings_advanced.model_storage_desc")}</p>
      <div>
        <ConfirmAction
          label={$t("settings_advanced.delete_model")}
          confirmLabel={$t("settings_advanced.delete_model_confirm")}
          confirmMessage={$t("settings_advanced.delete_model_msg")}
          variant="danger"
          onConfirm={controller.handleDeleteModel}
        />
      </div>
    </section>
  </AdvancedSettingsSection>

  <AboutSettingsSection
    selectedTranscriptionLabel={selectedTranscriptionLabel}
    selectedOllamaModelLabel={selectedOllamaModelLabel}
  />
</div>
