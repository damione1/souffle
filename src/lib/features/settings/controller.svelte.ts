import { getOllamaStatus } from "../../api/ollama";
import { getTranscriptionCatalog } from "../../api/transcription";
import {
  getSettings,
  getShortcuts,
  listAudioDevices,
  saveSettings,
  saveShortcuts as persistShortcutSettings,
  selectAudioDevice,
} from "../../api/settings";
import { getAppState } from "../../stores/app.svelte";
import type {
  AppSettings,
  AudioDeviceInfo,
  OllamaModelDescriptor,
  ShortcutSettings,
  Theme,
  TranscriptionCatalog,
} from "../../types";
import { applyTheme, errorMessage } from "../../utils";

export function createSettingsController() {
  const app = getAppState();

  let audioDevices = $state<AudioDeviceInfo[]>([]);
  let ollamaAvailable = $state(false);
  let ollamaModels = $state<OllamaModelDescriptor[]>([]);
  let statusMessage = $state("");
  let catalog = $state<TranscriptionCatalog | null>(null);

  let toggleShortcut = $state("CommandOrControl+Shift+Space");
  let pttShortcut = $state("");
  let recordingField = $state<"toggle" | "ptt" | null>(null);
  let shortcutError = $state("");

  let summaryModels = $derived(ollamaModels.filter((model) => model.can_summarize));

  async function mount() {
    await syncSettings();
    await Promise.all([
      loadShortcuts(),
      refreshDevices(),
      checkOllama(),
      loadCatalog(),
    ]);
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
      };
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
    const nextSettings: AppSettings = {
      ...app.settings,
      audio_device: app.selectedDevice || null,
    };
    updater(nextSettings);

    try {
      await saveSettings(nextSettings);
      app.settings = nextSettings;
      app.selectedDevice = nextSettings.audio_device ?? "";
      await loadCatalog();
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

  async function checkOllama() {
    try {
      const status = await getOllamaStatus();
      const availableSummaryModels = status.models.filter((model) => model.can_summarize);
      ollamaAvailable = status.available;
      ollamaModels = status.models;

      if (status.available && availableSummaryModels.length === 0 && status.models.length > 0) {
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
      ollamaModels = [];
    }
  }

  async function selectTranscriptionEngine(engineId: string) {
    const engine = catalog?.engines.find((candidate) => candidate.id === engineId);
    const modelId = engine?.models[0]?.id;
    if (!modelId) return;

    await persistSettings((settings) => {
      settings.transcription_engine_id = engineId;
      settings.transcription_model_id = modelId;
    });
  }

  async function selectTranscriptionModel(modelId: string) {
    await persistSettings((settings) => {
      settings.transcription_model_id = modelId;
    });
  }

  function onThemeChange(theme: Theme) {
    applyTheme(theme);
    void persistSettings((settings) => {
      settings.theme = theme;
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
    if (!shortcut) return "Not set";
    return shortcut
      .replace(/CommandOrControl/g, "\u2318")
      .replace(/Shift/g, "\u21E7")
      .replace(/Alt/g, "\u2325")
      .replace(/\+/g, " ");
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
    get ollamaAvailable() { return ollamaAvailable; },
    get ollamaModels() { return ollamaModels; },
    get summaryModels() { return summaryModels; },
    get statusMessage() { return statusMessage; },
    get catalog() { return catalog; },
    get toggleShortcut() { return toggleShortcut; },
    get pttShortcut() { return pttShortcut; },
    get recordingField() { return recordingField; },
    get shortcutError() { return shortcutError; },
    mount,
    refreshDevices,
    onDeviceChange,
    checkOllama,
    selectTranscriptionEngine,
    selectTranscriptionModel,
    onThemeChange,
    onAutoPasteChange,
    onPasteDelayChange,
    onDebugTranscriptionChange,
    onOllamaUrlChange,
    onOllamaModelChange,
    startRecording,
    handleKeyDown,
    clearShortcut,
    formatShortcut,
  };
}
