<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type { AudioDevice, OllamaStatus, Theme } from "../types";
  import { getAppState } from "../stores/app.svelte";

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
    await loadSettings();
    await loadShortcuts();
    await refreshDevices();
    await checkOllama();
  });

  async function loadSettings() {
    try {
      const settings = await invoke<Record<string, unknown>>("get_settings");
      if (settings.theme) {
        const theme = settings.theme as Theme;
        app.settings = { ...app.settings, theme };
        app.theme = theme;
        applyTheme(theme);
      }
      if (settings.auto_paste !== null && settings.auto_paste !== undefined) {
        app.settings = { ...app.settings, auto_paste: settings.auto_paste as boolean };
      }
      if (settings.paste_delay_ms !== null && settings.paste_delay_ms !== undefined) {
        app.settings = { ...app.settings, paste_delay_ms: settings.paste_delay_ms as number };
      }
      if (settings.debug_transcription !== null && settings.debug_transcription !== undefined) {
        app.settings = { ...app.settings, debug_transcription: settings.debug_transcription as boolean };
      }
      if (settings.ollama_url) {
        app.settings = { ...app.settings, ollama_url: settings.ollama_url as string };
      }
      if (settings.ollama_model) {
        app.settings = { ...app.settings, ollama_model: settings.ollama_model as string };
      }
      if (settings.audio_device) {
        app.selectedDevice = settings.audio_device as string;
      }
    } catch (e) {
      console.warn("Failed to load settings:", e);
    }
  }

  async function loadShortcuts() {
    try {
      const shortcuts = await invoke<{ toggle: string; push_to_talk: string }>("get_shortcuts");
      toggleShortcut = shortcuts.toggle;
      pttShortcut = shortcuts.push_to_talk;
    } catch (e) {
      console.warn("Failed to load shortcuts:", e);
    }
  }

  async function saveSetting(key: string, value: unknown) {
    try {
      await invoke("save_setting", { key, value });
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function refreshDevices() {
    try {
      audioDevices = await invoke("list_audio_devices");
      if (app.selectedDevice) {
        const exists = audioDevices.some((d) => d.name === app.selectedDevice);
        if (exists) {
          await invoke("select_audio_device", { deviceName: app.selectedDevice });
          return;
        }
      }
      const defaultDevice = audioDevices.find((d) => d.is_default);
      if (defaultDevice) {
        app.selectedDevice = defaultDevice.name;
        await invoke("select_audio_device", { deviceName: defaultDevice.name });
      }
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function onDeviceChange(event: Event) {
    const target = event.target as HTMLSelectElement;
    app.selectedDevice = target.value;
    try {
      await invoke("select_audio_device", { deviceName: app.selectedDevice });
      saveSetting("audio_device", app.selectedDevice);
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function checkOllama() {
    try {
      const status: OllamaStatus = await invoke("check_ollama");
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
          app.settings = { ...app.settings, ollama_model: nextModel };
          await saveSetting("ollama_model", nextModel);
        }
      }
    } catch {
      ollamaAvailable = false;
    }
  }

  function applyTheme(theme: Theme) {
    if (theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)) {
      document.documentElement.classList.add("dark");
      document.documentElement.classList.remove("light");
    } else {
      document.documentElement.classList.remove("dark");
      document.documentElement.classList.add("light");
    }
  }

  function onThemeChange(theme: Theme) {
    app.settings = { ...app.settings, theme };
    app.theme = theme;
    applyTheme(theme);
    saveSetting("theme", theme);
  }

  function onAutoPasteChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    app.settings = { ...app.settings, auto_paste: checked };
    saveSetting("auto_paste", checked);
  }

  function onPasteDelayChange(event: Event) {
    const value = parseInt((event.target as HTMLInputElement).value);
    app.settings = { ...app.settings, paste_delay_ms: value };
    saveSetting("paste_delay_ms", value);
  }

  function onDebugTranscriptionChange(event: Event) {
    const checked = (event.target as HTMLInputElement).checked;
    app.settings = { ...app.settings, debug_transcription: checked };
    saveSetting("debug_transcription", checked);
  }

  function onOllamaUrlChange(event: Event) {
    const value = (event.target as HTMLInputElement).value;
    app.settings = { ...app.settings, ollama_url: value };
    saveSetting("ollama_url", value);
  }

  function onOllamaModelChange(event: Event) {
    const value = (event.target as HTMLSelectElement).value;
    app.settings = { ...app.settings, ollama_model: value };
    saveSetting("ollama_model", value);
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
      await invoke("update_shortcuts", { toggleShortcut, pttShortcut });
    } catch (e) {
      shortcutError = String(e);
    }
  }

  async function clearShortcut(field: "toggle" | "ptt") {
    if (field === "toggle") toggleShortcut = "";
    else pttShortcut = "";
    await saveShortcuts();
  }
</script>

<svelte:window onkeydown={handleKeyDown} />

<div class="view">
  <h2>Settings</h2>

  {#if statusMessage}
    <div class="status-banner">
      <p class="text-sm">{statusMessage}</p>
    </div>
  {/if}

  <!-- Audio Configuration -->
  <section class="surface-card section">
    <h3>Audio Configuration</h3>
    <p class="text-secondary text-sm">Choose the active microphone or virtual device.</p>

    <div class="field-group">
      <span class="field-label">Input device</span>
      <div class="input-row">
        <select value={app.selectedDevice} onchange={onDeviceChange} class="field-select">
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

    <div class="setting-row disabled">
      <div>
        <span class="setting-label">Noise Reduction</span>
        <span class="text-sm text-muted">Reduce background noise during capture.</span>
      </div>
      <div class="coming-soon-row">
        <span class="pill pill-muted">Coming Soon</span>
        <input type="checkbox" disabled class="switch" />
      </div>
    </div>
  </section>

  <!-- Intelligence -->
  <section class="surface-card section">
    <h3>Intelligence</h3>
    <p class="text-secondary text-sm">Transcription engine and summarization model.</p>

    <div class="setting-row">
      <div>
        <span class="setting-label">Transcription engine</span>
        <span class="text-sm text-muted">Local on-device inference.</span>
      </div>
      <span class="pill pill-blue">Kyutai STT 1B (FR/EN)</span>
    </div>

    <div class="setting-row">
      <div>
        <span class="setting-label">Ollama URL</span>
      </div>
      <input type="text" value={app.settings.ollama_url} onchange={onOllamaUrlChange} class="field-input" style="max-width: 16rem;" />
    </div>

    <div class="setting-row">
      <div>
        <span class="setting-label">Connection status</span>
      </div>
      <div class="inline-row">
        <span class="status-dot" class:is-online={ollamaAvailable}></span>
        <span class="text-sm text-muted">{ollamaAvailable ? "Connected" : "Not available"}</span>
        <button onclick={checkOllama} class="btn">Retry</button>
      </div>
    </div>

    {#if ollamaAvailable && ollamaSummaryModels.length > 0}
      <div class="field-group">
        <span class="field-label">Summarization model</span>
        <select
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
      <div class="status-banner">
        <p class="text-sm">No summary-capable model found. Install a chat model to enable summaries.</p>
      </div>
    {/if}
  </section>

  <!-- Interface -->
  <section class="surface-card section">
    <h3>Interface</h3>

    <div class="setting-row">
      <div>
        <span class="setting-label">Theme</span>
      </div>
      <div class="theme-group">
        {#each ["dark", "light", "system"] as themeOption}
          <button
            onclick={() => onThemeChange(themeOption as Theme)}
            class="btn"
            class:btn-active={app.settings.theme === themeOption}
          >
            {themeOption.charAt(0).toUpperCase() + themeOption.slice(1)}
          </button>
        {/each}
      </div>
    </div>

    <div class="setting-row">
      <div>
        <span class="setting-label">Auto-paste after dictation</span>
        <span class="text-sm text-muted">Copies transcript and simulates Cmd+V when capture stops.</span>
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
      <div class="setting-row">
        <div>
          <span class="setting-label">Paste delay</span>
          <span class="text-sm text-muted">Milliseconds to wait before pasting. Requires Accessibility permission.</span>
        </div>
        <input
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

    <div class="setting-row">
      <div>
        <span class="setting-label">Toggle recording</span>
        <span class="text-sm text-muted">Press once to start or stop dictation.</span>
      </div>
      <div class="inline-row">
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

    <div class="setting-row">
      <div>
        <span class="setting-label">Push-to-talk</span>
        <span class="text-sm text-muted">Hold to record, release to stop.</span>
      </div>
      <div class="inline-row">
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
      <div class="status-banner danger">
        <p class="text-sm">{shortcutError}</p>
      </div>
    {/if}
  </section>

  <!-- Diagnostics -->
  <section class="surface-card section">
    <h3>Diagnostics</h3>

    <div class="setting-row">
      <div>
        <span class="setting-label">Verbose transcription logs</span>
        <span class="text-sm text-muted">Per-frame STT logging and debug audio capture.</span>
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
  <section class="surface-card section">
    <h3>About</h3>

    <div class="about-grid">
      <div class="about-row">
        <span class="text-muted text-sm">Version</span>
        <span class="text-sm">v0.1.0</span>
      </div>
      <div class="about-row">
        <span class="text-muted text-sm">Engine</span>
        <span class="text-sm">Kyutai STT 1B (FR/EN)</span>
      </div>
      <div class="about-row">
        <span class="text-muted text-sm">Privacy</span>
        <span class="text-sm">Everything runs locally.</span>
      </div>
    </div>
  </section>
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 0.875rem;
  }

  .text-secondary {
    color: var(--color-text-secondary);
  }

  .text-muted {
    color: var(--color-text-muted);
  }

  .text-sm {
    font-size: 0.8125rem;
  }

  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }

  .setting-row.disabled {
    opacity: 0.5;
  }

  .setting-label {
    font-size: 0.9375rem;
    font-weight: 500;
    color: var(--color-text-primary);
    display: block;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .input-row {
    display: flex;
    gap: 0.375rem;
    align-items: center;
  }

  .inline-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .coming-soon-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .theme-group {
    display: flex;
    gap: 0.25rem;
  }

  .btn-active {
    background: var(--color-accent-blue) !important;
    color: #fff !important;
    outline: none !important;
  }

  .status-banner {
    padding: 0.75rem 1rem;
    border-radius: var(--radius-default);
    background: var(--color-surface-3);
    outline: 1px solid var(--color-ghost-border);
  }

  .status-banner.danger {
    outline-color: color-mix(in srgb, var(--color-danger) 30%, transparent);
  }

  .about-grid {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .about-row {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
  }
</style>
