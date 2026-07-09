import { getSummaryProvidersStatus } from "../../api/summary";
import { deleteModel, getTranscriptionCatalog } from "../../api/transcription";
import {
  getSettings,
  getShortcuts,
  getSystemAudioSupport,
  isLaptop as checkIsLaptop,
  listAudioDevices,
  saveSettings,
  saveShortcuts as persistShortcutSettings,
  selectAudioDevice,
} from "../../api/settings";
import {
  listDictionary,
  addDictionaryEntry as apiAddDictionaryEntry,
  deleteDictionaryEntry as apiDeleteDictionaryEntry,
} from "../../api/dictionary";
import { listCalendars } from "../../api/calendar";
import { requestPermission } from "../../api/permissions";
import { setLocale } from "../../i18n";
import { getAppState } from "../../stores/app.svelte";
import type {
  AppSettings,
  AudioDeviceInfo,
  CalendarInfo,
  DictionaryEntry,
  PermState,
  SummaryModelDescriptor,
  ShortcutSettings,
  Theme,
  TranscriptionCatalog,
} from "../../types";
import { applyTheme, errorMessage, formatShortcutLabel } from "../../utils";
import {
  getFirstAvailableTranscriptionBackend,
  getFirstAvailableTranscriptionModel,
  listAvailableModelOptions,
  getSelectedTranscriptionBackend,
} from "../transcription/catalog";
import {
  currentTranscriptionSelection,
  refreshTranscriptionRuntimeStatus,
  resetTranscriptionRuntimeState,
  startTranscriptionModelDownload,
  startTranscriptionModelLoad,
} from "../transcription/runtime";

export function createSettingsController() {
  const app = getAppState();

  let audioDevices = $state<AudioDeviceInfo[]>([]);
  let systemAudioSupported = $state(false);
  // Gates the "microphone when lid is closed" picker: meaningless on a
  // desktop Mac, so it's hidden entirely rather than shown disabled.
  let isLaptop = $state(false);
  let ollamaAvailable = $state(false);
  let appleIntelligenceAvailable = $state(false);
  let ollamaModels = $state<SummaryModelDescriptor[]>([]);
  let statusMessage = $state("");
  let catalog = $state<TranscriptionCatalog | null>(null);

  let toggleShortcut = $state("CommandOrControl+Shift+Space");
  let pttShortcut = $state("");
  let recordingField = $state<"toggle" | "ptt" | null>(null);
  let shortcutError = $state("");

  let dictionaryEntries = $state<DictionaryEntry[]>([]);

  let calendars = $state<CalendarInfo[]>([]);
  let calendarPermission = $state<PermState>("unknown");

  let summaryModels = $derived(ollamaModels.filter((model) => model.can_summarize && model.provider === "ollama"));

  async function mount() {
    await syncSettings();
    getSystemAudioSupport()
      .then((supported) => { systemAudioSupported = supported; })
      .catch(() => { systemAudioSupported = false; });
    checkIsLaptop()
      .then((laptop) => { isLaptop = laptop; })
      .catch(() => { isLaptop = false; });
    await Promise.all([
      loadShortcuts(),
      refreshDevices(),
      refreshSummaryProviders(),
      loadCatalog(),
      loadDictionary(),
      loadCalendars(),
    ]);
    await refreshRuntimeStatus();
  }

  async function syncSettings() {
    try {
      const settings = await getSettings();
      app.settings = settings;
      applyTheme(app.settings.theme);
      app.selectedDevice = settings.audio_device ?? "";
    } catch (e) {
      console.warn("Failed to load settings:", e);
    }
  }

  async function loadCatalog() {
    try {
      catalog = await getTranscriptionCatalog();
      app.settings = {
        ...app.settings,
        transcription_engine_id: catalog.selected_engine_id,
        transcription_model_id: catalog.selected_model_id,
        transcription_backend_id: catalog.selected_backend_id,
      };
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function refreshRuntimeStatus() {
    try {
      await refreshTranscriptionRuntimeStatus(app, catalog);
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function loadShortcuts() {
    try {
      const shortcuts = await getShortcuts();
      toggleShortcut = shortcuts.toggle;
      pttShortcut = shortcuts.push_to_talk;
    } catch (e) {
      console.warn("Failed to load shortcuts:", e);
    }
  }

  async function persistSettings(updater: (settings: AppSettings) => void) {
    const previousSettings = app.settings;
    const nextSettings: AppSettings = {
      ...previousSettings,
      audio_device: app.selectedDevice || null,
    };
    updater(nextSettings);
    const selectionChanged =
      nextSettings.transcription_engine_id !== previousSettings.transcription_engine_id
      || nextSettings.transcription_model_id !== previousSettings.transcription_model_id
      || nextSettings.transcription_backend_id !== previousSettings.transcription_backend_id;

    try {
      await saveSettings(nextSettings);
      app.settings = nextSettings;
      app.selectedDevice = nextSettings.audio_device ?? "";
      if (selectionChanged) {
        resetTranscriptionRuntimeState(app);
        await loadCatalog();
        await refreshRuntimeStatus();
      }
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function refreshDevices() {
    try {
      audioDevices = await listAudioDevices();
      if (app.selectedDevice) {
        const exists = audioDevices.some((device) => device.name === app.selectedDevice);
        if (exists) {
          await selectAudioDevice(app.selectedDevice);
          return;
        }
      }
      const defaultDevice = audioDevices.find((device) => device.is_default);
      if (defaultDevice) {
        app.selectedDevice = defaultDevice.name;
        await selectAudioDevice(defaultDevice.name);
      }
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function onDeviceChange(event: Event) {
    const target = event.target as HTMLSelectElement;
    try {
      await selectAudioDevice(target.value);
      await persistSettings((settings) => {
        settings.audio_device = target.value;
      });
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /** Empty option value means "follow system default" (the previous,
   * only behavior) — persisted as `null`, matching the backend contract. */
  function onClamshellDeviceChange(event: Event) {
    const target = event.target as HTMLSelectElement;
    const value = target.value || null;
    void persistSettings((settings) => {
      settings.clamshell_audio_device = value;
    });
  }

  async function refreshSummaryProviders() {
    try {
      const status = await getSummaryProvidersStatus();
      ollamaAvailable = status.ollama_available;
      appleIntelligenceAvailable = status.apple_intelligence_available;
      ollamaModels = status.models.filter((model) => model.provider === "ollama");

      const availableSummaryModels = status.models.filter(
        (model) => model.can_summarize && model.provider === "ollama",
      );
      if (status.ollama_available && availableSummaryModels.length === 0 && ollamaModels.length > 0) {
        statusMessage = "No text-generation Ollama model found. Install a chat model (qwen, llama, mistral, etc.).";
        return;
      }

      if (availableSummaryModels.length > 0) {
        const nextModel = availableSummaryModels.some((model) => model.id === app.settings.ollama_model)
          ? app.settings.ollama_model
          : availableSummaryModels[0].id;
        if (nextModel && nextModel !== app.settings.ollama_model) {
          await persistSettings((settings) => {
            settings.ollama_model = nextModel;
          });
        }
      }
    } catch {
      ollamaAvailable = false;
      appleIntelligenceAvailable = false;
      ollamaModels = [];
    }
  }

  /** Simple-picker selection: persist the (engine, model, backend) triple,
   * then download (if needed) and load without further clicks. */
  async function selectModelOption(key: string) {
    const option = listAvailableModelOptions(catalog).find((candidate) => candidate.key === key);
    if (!option) return;

    await persistSettings((settings) => {
      settings.transcription_engine_id = option.engineId;
      settings.transcription_model_id = option.modelId;
      settings.transcription_backend_id = option.backendId;
    });

    if (app.transcriptionRuntimePhase === "download_required") {
      await startTranscriptionModelDownload(
        app,
        catalog,
        (message) => { statusMessage = message; },
        { autoLoad: true },
      );
    } else if (app.transcriptionRuntimePhase === "load_required") {
      await startTranscriptionModelLoad(app, catalog, (message) => {
        statusMessage = message;
      });
    }
  }

  async function selectTranscriptionEngine(engineId: string) {
    const engine = catalog?.engines.find((candidate) => candidate.id === engineId);
    const model = getFirstAvailableTranscriptionModel(engine ?? null);
    const backendId = getFirstAvailableTranscriptionBackend(model)?.id;
    if (!model || !backendId) return;

    await persistSettings((settings) => {
      settings.transcription_engine_id = engineId;
      settings.transcription_model_id = model.id;
      settings.transcription_backend_id = backendId;
    });
  }

  async function selectTranscriptionModel(modelId: string) {
    const backend = getSelectedTranscriptionBackend(
      catalog,
      app.settings.transcription_engine_id,
      modelId,
      app.settings.transcription_backend_id,
    );
    await persistSettings((settings) => {
      settings.transcription_model_id = modelId;
      settings.transcription_backend_id = backend?.id ?? settings.transcription_backend_id;
    });
  }

  async function selectTranscriptionBackend(backendId: string) {
    await persistSettings((settings) => {
      settings.transcription_backend_id = backendId;
    });
  }

  async function handleDownloadModel() {
    await startTranscriptionModelDownload(app, catalog, (message) => {
      statusMessage = message;
    });
  }

  async function handleLoadModel() {
    await startTranscriptionModelLoad(app, catalog, (message) => {
      statusMessage = message;
    });
  }

  async function handleDeleteModel() {
    try {
      const selection = currentTranscriptionSelection(app, catalog);
      await deleteModel(selection);
      resetTranscriptionRuntimeState(app);
      await refreshRuntimeStatus();
    } catch (error) {
      statusMessage = errorMessage(error);
    }
  }

  function onThemeChange(theme: Theme) {
    applyTheme(theme);
    void persistSettings((settings) => {
      settings.theme = theme;
    });
  }

  function onLocaleChange(locale: string) {
    setLocale(locale);
    void persistSettings((settings) => {
      settings.locale = locale;
    });
  }

  function onAutoPasteChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.auto_paste = checked;
    });
  }

  function onPasteDelayChange(event: Event) {
    const value = parseInt((event.target as HTMLInputElement).value, 10);
    void persistSettings((settings) => {
      settings.paste_delay_ms = value;
    });
  }

  function onDebugTranscriptionChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.debug_transcription = checked;
    });
  }

  function onOllamaUrlChange(event: Event) {
    const value = (event.target as HTMLInputElement).value;
    void persistSettings((settings) => {
      settings.ollama_url = value;
    });
  }

  function onOllamaModelChange(event: Event) {
    const value = (event.target as HTMLSelectElement).value;
    void persistSettings((settings) => {
      settings.ollama_model = value;
    });
  }

  function onVadEnabledChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.vad_enabled = checked;
    });
  }

  function onFillerRemovalChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.filler_removal = checked;
    });
  }

  function onStutterCollapseChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.stutter_collapse = checked;
    });
  }

  function onDictionaryCorrectionChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.dictionary_correction = checked;
    });
  }

  function onCaptureSystemAudioChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.capture_system_audio = checked;
    });
  }

  function onModelUnloadTimeoutChange(event: Event) {
    const value = parseInt((event.target as HTMLSelectElement).value, 10);
    if (!Number.isFinite(value)) return;
    void persistSettings((settings) => {
      settings.model_unload_timeout_minutes = value;
    });
  }

  function onMeetingAutostopEnabledChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    void persistSettings((settings) => {
      settings.meeting_autostop_enabled = checked;
    });
  }

  function onMeetingAutostopMinutesChange(event: Event) {
    const value = parseInt((event.target as HTMLSelectElement).value, 10);
    if (!Number.isFinite(value)) return;
    void persistSettings((settings) => {
      settings.meeting_autostop_minutes = value;
    });
  }

  function onMeetingMaxDurationMinutesChange(event: Event) {
    const value = parseInt((event.target as HTMLSelectElement).value, 10);
    if (!Number.isFinite(value)) return;
    void persistSettings((settings) => {
      settings.meeting_max_duration_minutes = value;
    });
  }

  /** Populate the calendar picker without prompting: only fetch calendars
   * when the integration is on (which implies access was granted). */
  async function loadCalendars() {
    if (!app.settings.calendar_integration_enabled) return;
    try {
      calendars = await listCalendars();
      calendarPermission = "granted";
    } catch (e) {
      calendars = [];
      calendarPermission = "denied";
      console.warn("Failed to load calendars:", e);
    }
  }

  /** Enabling triggers the TCC prompt (or opens System Settings after a
   * deny); the setting only persists as enabled when access is granted. */
  async function onCalendarEnabledChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    if (!checked) {
      await persistSettings((settings) => {
        settings.calendar_integration_enabled = false;
      });
      return;
    }
    try {
      calendarPermission = await requestPermission("calendar");
    } catch (e) {
      calendarPermission = "denied";
      statusMessage = errorMessage(e);
    }
    if (calendarPermission !== "granted") {
      (event.target as HTMLInputElement).checked = false;
      return;
    }
    await persistSettings((settings) => {
      settings.calendar_integration_enabled = true;
    });
    await loadCalendars();
  }

  /** Empty selection means "all calendars", so unchecking one while empty
   * materializes the full list first. A full selection collapses back to
   * empty, and the last checked calendar cannot be removed. */
  function toggleCalendarSelected(id: string) {
    const allIds = calendars.map((calendar) => calendar.id);
    void persistSettings((settings) => {
      const effective = settings.calendar_selected_ids.length === 0
        ? allIds
        : settings.calendar_selected_ids;
      const next = effective.includes(id)
        ? effective.filter((existing) => existing !== id)
        : [...effective, id];
      if (next.length === 0) return;
      settings.calendar_selected_ids = next.length === allIds.length ? [] : next;
    });
  }

  function onCalendarReminderMinutesChange(event: Event) {
    const value = parseInt((event.target as HTMLInputElement).value, 10);
    if (!Number.isFinite(value)) return;
    void persistSettings((settings) => {
      settings.calendar_reminder_minutes = value;
    });
  }

  async function loadDictionary() {
    try {
      dictionaryEntries = await listDictionary();
    } catch (e) {
      console.warn("Failed to load dictionary:", e);
    }
  }

  async function handleAddDictionaryEntry(term: string, pronunciation: string | null, category: string | null) {
    const entry = await apiAddDictionaryEntry(term, pronunciation, category);
    dictionaryEntries = [...dictionaryEntries, entry].sort((a, b) => a.term.localeCompare(b.term));
  }

  async function handleDeleteDictionaryEntry(id: number) {
    await apiDeleteDictionaryEntry(id);
    dictionaryEntries = dictionaryEntries.filter((e) => e.id !== id);
  }

  function keyEventToShortcut(event: KeyboardEvent): string | null {
    if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) return null;
    const parts: string[] = [];
    if (event.metaKey || event.ctrlKey) parts.push("CommandOrControl");
    if (event.shiftKey) parts.push("Shift");
    if (event.altKey) parts.push("Alt");
    const key = mapKey(event.code, event.key);
    if (!key) return null;
    parts.push(key);
    return parts.join("+");
  }

  function mapKey(code: string, key: string): string | null {
    if (/^F\d{1,2}$/.test(key)) return key;
    if (code.startsWith("Key")) return code.slice(3);
    if (code.startsWith("Digit")) return code.slice(5);
    const keyMap: Record<string, string> = {
      Space: "Space", Enter: "Enter", Escape: "Escape", Backspace: "Backspace",
      Tab: "Tab", ArrowUp: "ArrowUp", ArrowDown: "ArrowDown", ArrowLeft: "ArrowLeft",
      ArrowRight: "ArrowRight", Delete: "Delete", Home: "Home", End: "End",
      PageUp: "PageUp", PageDown: "PageDown", Backquote: "Backquote", Minus: "Minus",
      Equal: "Equal", BracketLeft: "BracketLeft", BracketRight: "BracketRight",
      Backslash: "Backslash", Semicolon: "Semicolon", Quote: "Quote",
      Comma: "Comma", Period: "Period", Slash: "Slash",
    };
    return keyMap[code] || null;
  }

  function formatShortcut(shortcut: string): string {
    return formatShortcutLabel(shortcut) || "Not set";
  }

  function startRecording(field: "toggle" | "ptt") {
    recordingField = field;
    shortcutError = "";
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (!recordingField) return;
    event.preventDefault();
    event.stopPropagation();

    if (event.key === "Escape") {
      recordingField = null;
      return;
    }

    if (event.key === "Backspace" || event.key === "Delete") {
      if (recordingField === "toggle") toggleShortcut = "";
      else pttShortcut = "";
      recordingField = null;
      void saveShortcutSettings();
      return;
    }

    const shortcut = keyEventToShortcut(event);
    if (!shortcut) return;

    if (!event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey && !/^F\d{1,2}$/.test(event.key)) {
      shortcutError = "Shortcut must include a modifier key (Cmd, Ctrl, Shift, Alt) or be a function key";
      return;
    }

    if (recordingField === "toggle") toggleShortcut = shortcut;
    else pttShortcut = shortcut;
    recordingField = null;
    void saveShortcutSettings();
  }

  async function saveShortcutSettings() {
    shortcutError = "";
    try {
      await persistShortcutSettings({
        toggle: toggleShortcut,
        push_to_talk: pttShortcut,
      } satisfies ShortcutSettings);
    } catch (e) {
      shortcutError = errorMessage(e);
    }
  }

  async function clearShortcut(field: "toggle" | "ptt") {
    if (field === "toggle") toggleShortcut = "";
    else pttShortcut = "";
    await saveShortcutSettings();
  }

  return {
    get app() { return app; },
    get audioDevices() { return audioDevices; },
    get appleIntelligenceAvailable() { return appleIntelligenceAvailable; },
    get ollamaAvailable() { return ollamaAvailable; },
    get ollamaModels() { return ollamaModels; },
    get summaryModels() { return summaryModels; },
    get statusMessage() { return statusMessage; },
    get catalog() { return catalog; },
    get runtimePhase() { return app.transcriptionRuntimePhase; },
    get modelOperationState() { return app.transcriptionModelOperationState; },
    get downloadFile() { return app.downloadFile; },
    get downloadCompletedFiles() { return app.downloadCompletedFiles; },
    get downloadTotalFiles() { return app.downloadTotalFiles; },
    get downloadedBytes() { return app.downloadedBytes; },
    get downloadTotalBytes() { return app.downloadTotalBytes; },
    get toggleShortcut() { return toggleShortcut; },
    get pttShortcut() { return pttShortcut; },
    get recordingField() { return recordingField; },
    get shortcutError() { return shortcutError; },
    get dictionaryEntries() { return dictionaryEntries; },
    get calendars() { return calendars; },
    get calendarPermission() { return calendarPermission; },
    onCalendarEnabledChange,
    toggleCalendarSelected,
    onCalendarReminderMinutesChange,
    mount,
    refreshRuntimeStatus,
    refreshDevices,
    onDeviceChange,
    refreshSummaryProviders,
    selectModelOption,
    handleDeleteModel,
    onThemeChange,
    onLocaleChange,
    onAutoPasteChange,
    onPasteDelayChange,
    onDebugTranscriptionChange,
    onOllamaUrlChange,
    onOllamaModelChange,
    get systemAudioSupported() { return systemAudioSupported; },
    get isLaptop() { return isLaptop; },
    onCaptureSystemAudioChange,
    onClamshellDeviceChange,
    onModelUnloadTimeoutChange,
    onMeetingAutostopEnabledChange,
    onMeetingAutostopMinutesChange,
    onMeetingMaxDurationMinutesChange,
    onVadEnabledChange,
    onFillerRemovalChange,
    onStutterCollapseChange,
    onDictionaryCorrectionChange,
    handleAddDictionaryEntry,
    handleDeleteDictionaryEntry,
    startRecording,
    handleKeyDown,
    clearShortcut,
    formatShortcut,
  };
}
