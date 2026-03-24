<script lang="ts">
  import { onMount } from "svelte";
  import {
    getSettings,
    getShortcuts,
    listAudioDevices,
    saveSettings,
    saveShortcuts as persistShortcutSettings,
    selectAudioDevice,
    toAppSettings,
    withAudioDevice,
  } from "../api/settings";
  import { getOllamaStatus } from "../api/ollama";
  import type {
    AudioDevice,
    PersistedAppSettings,
    ShortcutSettings,
    Theme,
  } from "../types";
  import { getAppState } from "../stores/app.svelte";
  import { applyTheme, errorMessage } from "../utils";
  import StatusBanner from "./ui/StatusBanner.svelte";

  const app = getAppState();

  let audioDevices = $state<AudioDevice[]>([]);
  let ollamaAvailable = $state(false);
  let ollamaSummaryModels = $state<string[]>([]);
  let statusMessage = $state("");

  let toggleShortcut = $state("CommandOrControl+Shift+Space");
  let pttShortcut = $state("");
  let recordingField = $state<"toggle" | "ptt" | null>(null);
  let shortcutError = $state("");

  onMount(async () => {
    await syncSettings();
    await loadShortcuts();
    await refreshDevices();
    await checkOllama();
  });

  async function syncSettings() {
    try {
      const settings = await getSettings();
      app.settings = toAppSettings(settings);
      applyTheme(app.settings.theme);
      if (settings.audio_device) {
        app.selectedDevice = settings.audio_device;
      }
    } catch (e) {
      console.warn("Failed to load settings:", e);
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

  async function persistSettings(
    updater: (settings: PersistedAppSettings) => void,
  ) {
    const nextSettings = withAudioDevice(app.settings, app.selectedDevice || null);
    updater(nextSettings);

    try {
      await saveSettings(nextSettings);
      app.settings = toAppSettings(nextSettings);
      app.selectedDevice = nextSettings.audio_device ?? "";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function refreshDevices() {
    try {
      audioDevices = await listAudioDevices();
      if (app.selectedDevice) {
        const exists = audioDevices.some((d) => d.name === app.selectedDevice);
        if (exists) {
          await selectAudioDevice(app.selectedDevice);
          return;
        }
      }
      const defaultDevice = audioDevices.find((d) => d.is_default);
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
      ollamaAvailable = status.available;
      ollamaSummaryModels = status.summary_models;

      if (status.available && status.summary_models.length === 0 && status.models.length > 0) {
        statusMessage = "No text-generation Ollama model found. Install a chat model (qwen, llama, mistral, etc.).";
        return;
      }

      if (status.summary_models.length > 0) {
        const nextModel = status.summary_models.includes(app.settings.ollama_model)
          ? app.settings.ollama_model
          : status.summary_models[0];
        if (nextModel && nextModel !== app.settings.ollama_model) {
          await persistSettings((settings) => {
            settings.ollama_model = nextModel;
          });
        }
      }
    } catch {
      ollamaAvailable = false;
    }
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
    const value = parseInt((event.target as HTMLInputElement).value);
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
      saveShortcuts();
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
    saveShortcuts();
  }

  async function saveShortcuts() {
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
    await saveShortcuts();
  }
</script>

<svelte:window onkeydown={handleKeyDown} />

<div class="flex flex-col gap-4">
  <h2>Settings</h2>

  {#if statusMessage}
    <StatusBanner message={statusMessage} />
  {/if}

  <!-- Audio Configuration -->
  <section class="surface-card flex flex-col gap-3.5">
    <h3>Audio Configuration</h3>
    <p class="text-text-secondary text-sm">Choose the active microphone or virtual device.</p>

    <div class="flex flex-col gap-1.5">
      <label for="input-device" class="field-label">Input device</label>
      <div class="flex gap-1.5 items-center">
        <select id="input-device" value={app.selectedDevice} onchange={onDeviceChange} class="field-select">
          {#each audioDevices as device}
            <option value={device.name}>
              {device.name}{device.is_default ? " (default)" : ""}
            </option>
          {/each}
        </select>
        <button onclick={refreshDevices} class="btn btn-icon" aria-label="Refresh devices">
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16">
            <path fill-rule="evenodd" d="M15.312 11.424a5.5 5.5 0 0 1-9.201 2.466l-.312-.311h2.433a.75.75 0 0 0 0-1.5H4.598a.75.75 0 0 0-.75.75v3.634a.75.75 0 0 0 1.5 0v-2.033l.312.311a7 7 0 0 0 11.712-3.138.75.75 0 0 0-1.449-.389Zm-11.23-3.27a.75.75 0 0 0 1.449.39A5.5 5.5 0 0 1 14.7 6.079l.312.311H12.78a.75.75 0 0 0 0 1.5h3.634a.75.75 0 0 0 .75-.75V3.506a.75.75 0 0 0-1.5 0v2.033l-.312-.311A7 7 0 0 0 3.693 8.343Z" clip-rule="evenodd" />
          </svg>
        </button>
      </div>
    </div>

    <div class="flex items-center justify-between gap-4 opacity-50">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Noise Reduction</span>
        <span class="text-sm text-text-muted">Reduce background noise during capture.</span>
      </div>
      <div class="flex gap-2 items-center">
        <span class="pill pill-muted">Coming Soon</span>
        <input type="checkbox" disabled class="switch" />
      </div>
    </div>
  </section>

  <!-- Intelligence -->
  <section class="surface-card flex flex-col gap-3.5">
    <h3>Intelligence</h3>
    <p class="text-text-secondary text-sm">Transcription engine and summarization model.</p>

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Transcription engine</span>
        <span class="text-sm text-text-muted">Local on-device inference.</span>
      </div>
      <span class="pill pill-blue">Kyutai STT 1B (FR/EN)</span>
    </div>

    <div class="flex items-center justify-between gap-4">
      <div>
        <label for="ollama-url" class="block text-[0.9375rem] font-medium text-text-primary">Ollama URL</label>
      </div>
      <input id="ollama-url" type="text" value={app.settings.ollama_url} onchange={onOllamaUrlChange} class="field-input max-w-64" />
    </div>

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Connection status</span>
      </div>
      <div class="flex gap-2 items-center">
        <span class="status-dot" class:is-online={ollamaAvailable}></span>
        <span class="text-sm text-text-muted">{ollamaAvailable ? "Connected" : "Not available"}</span>
        <button onclick={checkOllama} class="btn">Retry</button>
      </div>
    </div>

    {#if ollamaAvailable && ollamaSummaryModels.length > 0}
      <div class="flex flex-col gap-1.5">
        <label for="summary-model" class="field-label">Summarization model</label>
        <select
          id="summary-model"
          value={app.settings.ollama_model || ollamaSummaryModels[0]}
          onchange={onOllamaModelChange}
          class="field-select"
        >
          {#each ollamaSummaryModels as model}
            <option value={model}>{model}</option>
          {/each}
        </select>
      </div>
    {:else if ollamaAvailable}
      <StatusBanner message="No summary-capable model found. Install a chat model to enable summaries." />
    {/if}
  </section>

  <!-- Interface -->
  <section class="surface-card flex flex-col gap-3.5">
    <h3>Interface</h3>

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Theme</span>
      </div>
      <div class="flex gap-1">
        {#each ["dark", "light", "system"] as themeOption}
          <button
            onclick={() => onThemeChange(themeOption as Theme)}
            class={`btn ${app.settings.theme === themeOption ? "btn-active" : ""}`}
          >
            {themeOption.charAt(0).toUpperCase() + themeOption.slice(1)}
          </button>
        {/each}
      </div>
    </div>

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Auto-paste after dictation</span>
        <span class="text-sm text-text-muted">Copies transcript and simulates Cmd+V when capture stops.</span>
      </div>
      <input
        type="checkbox"
        checked={app.settings.auto_paste}
        onchange={onAutoPasteChange}
        class="switch"
        aria-label="Auto-paste after dictation"
      />
    </div>

    {#if app.settings.auto_paste}
      <div class="flex items-center justify-between gap-4">
        <div>
          <label for="paste-delay" class="block text-[0.9375rem] font-medium text-text-primary">Paste delay</label>
          <span class="text-sm text-text-muted">Milliseconds to wait before pasting. Requires Accessibility permission.</span>
        </div>
        <input
          id="paste-delay"
          type="number"
          value={app.settings.paste_delay_ms}
          onchange={onPasteDelayChange}
          min="50"
          max="1000"
          step="50"
          class="field-number"
        />
      </div>
    {/if}

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Toggle recording</span>
        <span class="text-sm text-text-muted">Press once to start or stop dictation.</span>
      </div>
      <div class="flex gap-2 items-center">
        <button
          onclick={() => startRecording("toggle")}
          class="shortcut-button"
          class:is-recording={recordingField === "toggle"}
        >
          {recordingField === "toggle" ? "Press keys..." : formatShortcut(toggleShortcut)}
        </button>
        {#if toggleShortcut}
          <button onclick={() => clearShortcut("toggle")} class="btn btn-ghost text-sm">Clear</button>
        {/if}
      </div>
    </div>

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Push-to-talk</span>
        <span class="text-sm text-text-muted">Hold to record, release to stop.</span>
      </div>
      <div class="flex gap-2 items-center">
        <button
          onclick={() => startRecording("ptt")}
          class="shortcut-button"
          class:is-recording={recordingField === "ptt"}
        >
          {recordingField === "ptt" ? "Press keys..." : formatShortcut(pttShortcut)}
        </button>
        {#if pttShortcut}
          <button onclick={() => clearShortcut("ptt")} class="btn btn-ghost text-sm">Clear</button>
        {/if}
      </div>
    </div>

    {#if shortcutError}
      <StatusBanner message={shortcutError} variant="danger" />
    {/if}
  </section>

  <!-- Diagnostics -->
  <section class="surface-card flex flex-col gap-3.5">
    <h3>Diagnostics</h3>

    <div class="flex items-center justify-between gap-4">
      <div>
        <span class="block text-[0.9375rem] font-medium text-text-primary">Verbose transcription logs</span>
        <span class="text-sm text-text-muted">Per-frame STT logging and debug audio capture.</span>
      </div>
      <input
        type="checkbox"
        checked={app.settings.debug_transcription}
        onchange={onDebugTranscriptionChange}
        class="switch"
        aria-label="Verbose transcription debug logs"
      />
    </div>
  </section>

  <!-- About -->
  <section class="surface-card flex flex-col gap-3.5">
    <h3>About</h3>

    <div class="flex flex-col gap-2">
      <div class="flex justify-between gap-4">
        <span class="text-text-muted text-sm">Version</span>
        <span class="text-sm">v0.1.0</span>
      </div>
      <div class="flex justify-between gap-4">
        <span class="text-text-muted text-sm">Engine</span>
        <span class="text-sm">Kyutai STT 1B (FR/EN)</span>
      </div>
      <div class="flex justify-between gap-4">
        <span class="text-text-muted text-sm">Privacy</span>
        <span class="text-sm">Everything runs locally.</span>
      </div>
    </div>
  </section>
</div>
