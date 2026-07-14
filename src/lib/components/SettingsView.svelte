<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import AboutSettingsSection from "../features/settings/components/AboutSettingsSection.svelte";
  import AdvancedSettingsSection from "../features/settings/components/AdvancedSettingsSection.svelte";
  import AudioSettingsSection from "../features/settings/components/AudioSettingsSection.svelte";
  import CalendarSettingsSection from "../features/settings/components/CalendarSettingsSection.svelte";
  import DataSettingsSection from "../features/settings/components/DataSettingsSection.svelte";
  import DictionarySettingsSection from "../features/settings/components/DictionarySettingsSection.svelte";
  import DiagnosticsSettingsSection from "../features/settings/components/DiagnosticsSettingsSection.svelte";
  import IntelligenceSettingsSection from "../features/settings/components/IntelligenceSettingsSection.svelte";
  import InterfaceSettingsSection from "../features/settings/components/InterfaceSettingsSection.svelte";
  import DictationPolishSettingsSection from "../features/settings/components/DictationPolishSettingsSection.svelte";
  import FeedbackSoundsSettingsSection from "../features/settings/components/FeedbackSoundsSettingsSection.svelte";
  import MicrophoneSettingsSection from "../features/settings/components/MicrophoneSettingsSection.svelte";
  import ModelSettingsSection from "../features/settings/components/ModelSettingsSection.svelte";
  import PermissionsSettingsSection from "../features/settings/components/PermissionsSettingsSection.svelte";
  import SummaryTemplatesSettingsSection from "../features/settings/components/SummaryTemplatesSettingsSection.svelte";
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

<svelte:window
  onkeydown={(event) => {
    controller.handleKeyDown(event);
    // Escape closes the settings screen unless it just cancelled a
    // shortcut-recording session (handleKeyDown preventDefaults those).
    if (!event.defaultPrevented && event.key === "Escape") {
      controller.app.settingsOpen = false;
    }
  }}
/>

<div class="flex flex-col gap-[22px]">
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
    unloadTimeoutMinutes={controller.app.settings.model_unload_timeout_minutes}
    onSelectModel={controller.selectModelOption}
    onUnloadTimeoutChange={controller.onModelUnloadTimeoutChange}
  />

  <MicrophoneSettingsSection
    audioDevices={controller.audioDevices}
    selectedDevice={controller.app.selectedDevice}
    onDeviceChange={controller.onDeviceChange}
    onRefreshDevices={controller.refreshDevices}
  />

  <InterfaceSettingsSection
    theme={controller.app.settings.theme}
    locale={controller.app.settings.locale}
    autoPaste={controller.app.settings.auto_paste}
    pasteDelayMs={controller.app.settings.paste_delay_ms}
    pasteMethod={controller.app.settings.paste_method}
    toggleShortcut={controller.toggleShortcut}
    pttShortcut={controller.pttShortcut}
    recordingField={controller.recordingField}
    shortcutError={controller.shortcutError}
    onThemeChange={controller.onThemeChange}
    onLocaleChange={controller.onLocaleChange}
    onAutoPasteChange={controller.onAutoPasteChange}
    onPasteDelayChange={controller.onPasteDelayChange}
    onPasteMethodChange={controller.onPasteMethodChange}
    onStartRecording={controller.startRecording}
    onClearShortcut={controller.clearShortcut}
    formatShortcut={controller.formatShortcut}
  />

  <DictationPolishSettingsSection
    enabled={controller.app.settings.dictation_polish_enabled}
    templateId={controller.app.settings.dictation_polish_template_id}
    templates={controller.app.settings.dictation_polish_templates}
    providerAvailable={controller.summaryProviderAvailable}
    onEnabledChange={controller.onDictationPolishEnabledChange}
    onTemplateChange={controller.onDictationPolishTemplateChange}
    onPromptChange={controller.onDictationPolishPromptChange}
  />

  <FeedbackSoundsSettingsSection
    enabled={controller.app.settings.feedback_sounds_enabled}
    volume={controller.app.settings.feedback_sounds_volume}
    onEnabledChange={controller.onFeedbackSoundsEnabledChange}
    onVolumeChange={controller.onFeedbackSoundsVolumeChange}
  />

  <DictionarySettingsSection
    entries={controller.dictionaryEntries}
    onAdd={controller.handleAddDictionaryEntry}
    onDelete={controller.handleDeleteDictionaryEntry}
  />

  <CalendarSettingsSection
    enabled={controller.app.settings.calendar_integration_enabled}
    permission={controller.calendarPermission}
    calendars={controller.calendars}
    selectedIds={controller.app.settings.calendar_selected_ids}
    reminderMinutes={controller.app.settings.calendar_reminder_minutes}
    autostartEnabled={controller.app.settings.calendar_autostart_enabled}
    onEnabledChange={controller.onCalendarEnabledChange}
    onToggleCalendar={controller.toggleCalendarSelected}
    onReminderMinutesChange={controller.onCalendarReminderMinutesChange}
    onAutostartEnabledChange={controller.onCalendarAutostartEnabledChange}
  />

  <AdvancedSettingsSection>
    <AudioSettingsSection
      audioDevices={controller.audioDevices}
      captureSystemAudio={controller.app.settings.capture_system_audio}
      systemAudioSupported={controller.systemAudioSupported}
      isLaptop={controller.isLaptop}
      clamshellAudioDevice={controller.app.settings.clamshell_audio_device}
      vadEnabled={controller.app.settings.vad_enabled}
      fillerRemoval={controller.app.settings.filler_removal}
      stutterCollapse={controller.app.settings.stutter_collapse}
      dictionaryCorrection={controller.app.settings.dictionary_correction}
      meetingAutostopEnabled={controller.app.settings.meeting_autostop_enabled}
      meetingAutostopMinutes={controller.app.settings.meeting_autostop_minutes}
      meetingMaxDurationMinutes={controller.app.settings.meeting_max_duration_minutes}
      diarizeEnabled={controller.app.settings.diarize_enabled}
      diarizeMaxSpeakers={controller.app.settings.diarize_max_speakers}
      diarizeDownloadState={controller.diarizeDownloadState}
      diarizeDownloadedBytes={controller.diarizeDownloadedBytes}
      diarizeDownloadTotalBytes={controller.diarizeDownloadTotalBytes}
      onCaptureSystemAudioChange={controller.onCaptureSystemAudioChange}
      onClamshellDeviceChange={controller.onClamshellDeviceChange}
      onVadEnabledChange={controller.onVadEnabledChange}
      onFillerRemovalChange={controller.onFillerRemovalChange}
      onStutterCollapseChange={controller.onStutterCollapseChange}
      onDictionaryCorrectionChange={controller.onDictionaryCorrectionChange}
      onMeetingAutostopEnabledChange={controller.onMeetingAutostopEnabledChange}
      onMeetingAutostopMinutesChange={controller.onMeetingAutostopMinutesChange}
      onMeetingMaxDurationMinutesChange={controller.onMeetingMaxDurationMinutesChange}
      onDiarizeEnabledChange={controller.onDiarizeEnabledChange}
      onDiarizeMaxSpeakersChange={controller.onDiarizeMaxSpeakersChange}
    />

    <IntelligenceSettingsSection
      ollamaUrl={controller.app.settings.ollama_url}
      ollamaAvailable={controller.ollamaAvailable}
      appleIntelligenceAvailable={controller.appleIntelligenceAvailable}
      appleIntelligenceUnavailableReason={controller.appleIntelligenceUnavailableReason}
      ollamaModels={controller.ollamaModels}
      summaryModels={controller.summaryModels}
      selectedOllamaModel={controller.app.settings.ollama_model}
      onOllamaUrlChange={controller.onOllamaUrlChange}
      onOllamaModelChange={controller.onOllamaModelChange}
      onRetrySummaryProviders={controller.refreshSummaryProviders}
    />

    <SummaryTemplatesSettingsSection
      templates={controller.app.settings.summary_templates}
      defaultTemplateId={controller.app.settings.default_summary_template_id}
      onDefaultChange={controller.onDefaultSummaryTemplateChange}
      onNameChange={controller.onSummaryTemplateNameChange}
      onPromptChange={controller.onSummaryTemplatePromptChange}
      onAdd={controller.addSummaryTemplate}
      onDelete={controller.deleteSummaryTemplate}
    />

    <PermissionsSettingsSection />

    <DiagnosticsSettingsSection
      debugTranscription={controller.app.settings.debug_transcription}
      logLevel={controller.app.settings.log_level}
      onDebugTranscriptionChange={controller.onDebugTranscriptionChange}
      onLogLevelChange={controller.onLogLevelChange}
    />

    <DataSettingsSection
      retention={controller.app.settings.meeting_audio_retention}
      onRetentionChange={controller.onMeetingAudioRetentionChange}
    />

    <section class="settings-group">
      <h3>{$t("settings_advanced.model_storage")}</h3>
      <div class="settings-rows">
        <div class="flex items-center justify-between gap-4">
          <span class="setting-desc min-w-0 flex-1">{$t("settings_advanced.model_storage_desc")}</span>
          <ConfirmAction
            label={$t("settings_advanced.delete_model")}
            confirmLabel={$t("settings_advanced.delete_model_confirm")}
            confirmMessage={$t("settings_advanced.delete_model_msg")}
            variant="danger"
            onConfirm={controller.handleDeleteModel}
          />
        </div>
      </div>
    </section>
  </AdvancedSettingsSection>

  <AboutSettingsSection
    selectedTranscriptionLabel={selectedTranscriptionLabel}
    selectedOllamaModelLabel={selectedOllamaModelLabel}
  />
</div>
