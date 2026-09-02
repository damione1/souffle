import { getAppState } from "../../stores/app.svelte";
import { getTranscriptionCatalog } from "../../api/transcription";
import {
  getShortcuts,
  listAudioDevices,
  saveSettings,
  saveShortcuts,
  selectAudioDevice,
} from "../../api/settings";
import { setLocale } from "../../i18n";
import type { AppSettings, AudioInputDevice, TranscriptionCatalog } from "../../types";
import { errorMessage, formatShortcutLabel } from "../../utils";
import { keyEventToShortcut, shortcutMissingModifier } from "../../utils/shortcut";
import { listAvailableModelOptions } from "../transcription/catalog";
import {
  resetTranscriptionRuntimeState,
  startTranscriptionModelDownload,
} from "../transcription/runtime";
import {
  markSetupComplete,
  readSetupFlags,
  wizardSteps,
  type SetupStep,
} from "./setup";

export function createOnboardingController() {
  const app = getAppState();

  let catalog = $state<TranscriptionCatalog | null>(null);
  let selectedKey = $state("");
  let statusMessage = $state("");
  let isStarting = $state(false);

  let steps = $state<SetupStep[]>(wizardSteps(readSetupFlags()));
  let stepIndex = $state(0);
  let recoveryOnly = $state(readSetupFlags().setupDone);

  let audioDevices = $state<AudioInputDevice[]>([]);
  let selectedDevice = $state("");

  let toggleShortcut = $state("CommandOrControl+Shift+Space");
  let pushToTalk = $state("");
  let rewrite = $state("");
  let recordingShortcut = $state(false);
  let shortcutError = $state("");
  let autoPaste = $state(true);

  const options = $derived(listAvailableModelOptions(catalog));
  const step = $derived(steps[stepIndex] ?? "permissions");
  const isDownloading = $derived(app.transcriptionModelOperationState === "downloading");
  const isLoading = $derived(app.transcriptionModelOperationState === "loading");
  const modelReady = $derived(app.transcriptionRuntimePhase === "ready");
  const busy = $derived(isStarting || isDownloading || isLoading);

  const canContinue = $derived.by(() => {
    switch (step) {
      case "permissions":
      case "microphone":
      case "shortcut":
        return true;
      case "model":
        return modelReady && !busy;
    }
  });

  async function persistSettings(updater: (settings: AppSettings) => void) {
    const nextSettings: AppSettings = { ...app.settings };
    updater(nextSettings);
    await saveSettings(nextSettings);
    app.settings = nextSettings;
  }

  async function mount() {
    const flags = readSetupFlags();
    recoveryOnly = flags.setupDone;
    steps = wizardSteps(flags);
    stepIndex = 0;
    selectedDevice = app.selectedDevice;
    autoPaste = true;

    try {
      catalog = await getTranscriptionCatalog();
      selectedKey = `${catalog.selected_engine_id}:${catalog.selected_model_id}`;
      if (!options.some((option) => option.key === selectedKey)) {
        selectedKey = options[0]?.key ?? "";
      }
    } catch (e) {
      statusMessage = errorMessage(e);
    }

    try {
      audioDevices = await listAudioDevices();
    } catch (e) {
      statusMessage = errorMessage(e);
    }

    try {
      const shortcuts = await getShortcuts();
      toggleShortcut = shortcuts.toggle || "CommandOrControl+Shift+Space";
      pushToTalk = shortcuts.push_to_talk;
      rewrite = shortcuts.rewrite;
    } catch {
      // Keep the built-in default.
    }
  }

  async function persistDevice(uid: string) {
    try {
      await selectAudioDevice(uid);
      app.selectedDevice = uid;
      await persistSettings((settings) => {
        settings.audio_device = uid || null;
      });
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function onDeviceChange(event: Event) {
    const uid = (event.currentTarget as HTMLSelectElement).value;
    selectedDevice = uid;
    await persistDevice(uid);
  }

  async function refreshDevices() {
    try {
      audioDevices = await listAudioDevices();
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function onLocaleChange(locale: string) {
    setLocale(locale);
    try {
      await persistSettings((settings) => {
        settings.locale = locale;
      });
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /** Persist the chosen model, then download and load it in one go. */
  async function beginDownload() {
    const option = options.find((candidate) => candidate.key === selectedKey);
    if (!option || isStarting || busy) return;

    isStarting = true;
    statusMessage = "";
    try {
      await persistSettings((settings) => {
        settings.transcription_engine_id = option.engineId;
        settings.transcription_model_id = option.modelId;
        settings.transcription_backend_id = option.backendId;
      });
      resetTranscriptionRuntimeState(app);

      await startTranscriptionModelDownload(
        app,
        catalog,
        (message) => {
          statusMessage = message;
        },
        { autoLoad: true },
      );
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isStarting = false;
    }
  }

  function startShortcutRecording() {
    recordingShortcut = true;
    shortcutError = "";
  }

  async function persistToggleShortcut(value: string) {
    shortcutError = "";
    toggleShortcut = value;
    try {
      await saveShortcuts({
        toggle: value,
        push_to_talk: pushToTalk,
        rewrite,
      });
    } catch (e) {
      shortcutError = errorMessage(e);
    }
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (!recordingShortcut) return;
    event.preventDefault();
    event.stopPropagation();

    if (event.key === "Escape") {
      recordingShortcut = false;
      return;
    }

    if (event.key === "Backspace" || event.key === "Delete") {
      recordingShortcut = false;
      void persistToggleShortcut("");
      return;
    }

    const shortcut = keyEventToShortcut(event);
    if (!shortcut) return;

    if (shortcutMissingModifier(event)) {
      shortcutError = "modifier";
      return;
    }

    recordingShortcut = false;
    void persistToggleShortcut(shortcut);
  }

  async function goNext() {
    if (!canContinue) return;
    if (stepIndex >= steps.length - 1) {
      await finish();
      return;
    }
    stepIndex += 1;
  }

  function goBack() {
    if (stepIndex === 0) return;
    recordingShortcut = false;
    stepIndex -= 1;
  }

  async function finish() {
    try {
      if (!recoveryOnly) {
        await persistSettings((settings) => {
          settings.auto_paste = autoPaste;
          settings.audio_device = selectedDevice || null;
        });
      }
    } catch (e) {
      statusMessage = errorMessage(e);
      return;
    }
    markSetupComplete();
    app.showOnboarding = false;
  }

  function formatShortcut(shortcut: string): string {
    return formatShortcutLabel(shortcut);
  }

  return {
    get app() { return app; },
    get options() { return options; },
    get selectedKey() { return selectedKey; },
    set selectedKey(key: string) { selectedKey = key; },
    get statusMessage() { return statusMessage; },
    get isStarting() { return isStarting; },
    get steps() { return steps; },
    get step() { return step; },
    get stepIndex() { return stepIndex; },
    get recoveryOnly() { return recoveryOnly; },
    get audioDevices() { return audioDevices; },
    get selectedDevice() { return selectedDevice; },
    get toggleShortcut() { return toggleShortcut; },
    get recordingShortcut() { return recordingShortcut; },
    get shortcutError() { return shortcutError; },
    get autoPaste() { return autoPaste; },
    set autoPaste(value: boolean) { autoPaste = value; },
    get busy() { return busy; },
    get isDownloading() { return isDownloading; },
    get isLoading() { return isLoading; },
    get modelReady() { return modelReady; },
    get canContinue() { return canContinue; },
    mount,
    beginDownload,
    onDeviceChange,
    refreshDevices,
    onLocaleChange,
    startShortcutRecording,
    handleKeyDown,
    goNext,
    goBack,
    finish,
    formatShortcut,
  };
}
