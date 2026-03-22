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
        const exists = audioDevices.some((device) => device.name === app.selectedDevice);
        if (exists) {
          await invoke("select_audio_device", { deviceName: app.selectedDevice });
          return;
        }
      }
      const defaultDevice = audioDevices.find((device) => device.is_default);
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
        statusMessage =
          "No text-generation Ollama model available for summaries. Install a chat model such as qwen, llama, mistral, gemma, phi, or deepseek.";
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
      Space: "Space",
      Enter: "Enter",
      Escape: "Escape",
      Backspace: "Backspace",
      Tab: "Tab",
      ArrowUp: "ArrowUp",
      ArrowDown: "ArrowDown",
      ArrowLeft: "ArrowLeft",
      ArrowRight: "ArrowRight",
      Delete: "Delete",
      Home: "Home",
      End: "End",
      PageUp: "PageUp",
      PageDown: "PageDown",
      Backquote: "Backquote",
      Minus: "Minus",
      Equal: "Equal",
      BracketLeft: "BracketLeft",
      BracketRight: "BracketRight",
      Backslash: "Backslash",
      Semicolon: "Semicolon",
      Quote: "Quote",
      Comma: "Comma",
      Period: "Period",
      Slash: "Slash",
    };

    return keyMap[code] || null;
  }

  function formatShortcut(shortcut: string): string {
    if (!shortcut) return "Not set";
    return shortcut
      .replace(/CommandOrControl/g, "⌘")
      .replace(/Shift/g, "⇧")
      .replace(/Alt/g, "⌥")
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
      await invoke("update_shortcuts", {
        toggleShortcut,
        pttShortcut,
      });
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

<div class="view-stack">
  <section class="surface-card surface-card--compact">
    <div class="panel-header">
      <div>
        <p class="eyebrow">Settings</p>
        <h3 class="section-title">Preferences</h3>
        <p class="helper-text">Shortcuts, audio, automation, diagnostics, and theme.</p>
      </div>

      <div class="action-row" style="flex-wrap: wrap;">
        <span class="pill">{app.settings.theme} theme</span>
        <span class="pill">{app.selectedDevice || "System default input"}</span>
        <span class={`pill ${ollamaAvailable ? "pill--success" : "pill--warning"}`}>
          {ollamaAvailable ? "Ollama ready" : "Ollama offline"}
        </span>
      </div>
    </div>
  </section>

  {#if statusMessage}
    <div class="status-banner status-banner--warning">
      <strong>Status</strong>
      <p class="helper-text">{statusMessage}</p>
    </div>
  {/if}

  <div class="settings-grid">
    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">Keyboard</p>
          <h3 class="section-title">Global shortcuts</h3>
          <p class="section-description">Capture reliable key chords for fast start/stop workflows and push-to-talk.</p>
        </div>

        <div class="settings-list">
          <div class="shortcut-row">
            <div class="shortcut-meta">
              <span class="shortcut-label">Toggle recording</span>
              <span class="shortcut-help">Press once to start or stop dictation.</span>
            </div>
            <div class="action-row" style="flex-wrap: wrap; justify-content: flex-end;">
              <button
                onclick={() => startRecording("toggle")}
                class="shortcut-button"
                class:is-recording={recordingField === "toggle"}
              >
                {recordingField === "toggle" ? "Press keys..." : formatShortcut(toggleShortcut)}
              </button>
              {#if toggleShortcut}
                <button onclick={() => clearShortcut("toggle")} class="button button-text">Clear</button>
              {/if}
            </div>
          </div>

          <div class="shortcut-row">
            <div class="shortcut-meta">
              <span class="shortcut-label">Push-to-talk</span>
              <span class="shortcut-help">Hold to record, release to stop.</span>
            </div>
            <div class="action-row" style="flex-wrap: wrap; justify-content: flex-end;">
              <button
                onclick={() => startRecording("ptt")}
                class="shortcut-button"
                class:is-recording={recordingField === "ptt"}
              >
                {recordingField === "ptt" ? "Press keys..." : formatShortcut(pttShortcut)}
              </button>
              {#if pttShortcut}
                <button onclick={() => clearShortcut("ptt")} class="button button-text">Clear</button>
              {/if}
            </div>
          </div>
        </div>

        {#if shortcutError}
          <div class="status-banner status-banner--danger">
            <strong>Shortcut error</strong>
            <p class="helper-text">{shortcutError}</p>
          </div>
        {/if}

        <p class="helper-text">Click a shortcut, then press the desired key combination. Press Escape to cancel or Delete to clear it.</p>
      </div>
    </section>

    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">Audio</p>
          <h3 class="section-title">Input routing</h3>
          <p class="section-description">Choose the active microphone or virtual device used by dictation and meeting recording.</p>
        </div>

        <label class="field-group">
          <span class="field-label">Input device</span>
          <select value={app.selectedDevice} onchange={onDeviceChange} class="field-select">
            {#each audioDevices as device}
              <option value={device.name}>
                {device.name}{device.is_default ? " (default)" : ""}
              </option>
            {/each}
          </select>
        </label>

        <div class="action-row">
          <button onclick={refreshDevices} class="button button-secondary">Refresh devices</button>
          <span class="helper-text">Use BlackHole or another loopback device for system audio capture.</span>
        </div>
      </div>
    </section>

    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">Dictation</p>
          <h3 class="section-title">Post-processing behavior</h3>
          <p class="section-description">Keep the automation explicit so it is easy to predict how text will be inserted.</p>
        </div>

        <div class="setting-row">
          <div class="shortcut-meta">
            <span class="shortcut-label">Auto-paste after dictation</span>
            <span class="shortcut-help">Copies the transcript and simulates Cmd+V when capture stops.</span>
          </div>
          <input
            type="checkbox"
            checked={app.settings.auto_paste}
            onchange={onAutoPasteChange}
            class="switch"
            aria-label="Auto-paste after dictation"
          />
        </div>

        <label class="field-group">
          <span class="field-label">Paste delay</span>
          <input
            type="number"
            value={app.settings.paste_delay_ms}
            onchange={onPasteDelayChange}
            min="50"
            max="1000"
            step="50"
            class="field-number"
          />
          <p class="helper-text">Short delays work best when the target app is already focused. Accessibility permission is still required.</p>
        </label>
      </div>
    </section>

    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">Diagnostics</p>
          <h3 class="section-title">Troubleshooting controls</h3>
          <p class="section-description">Expose debug output only when you need to inspect the transcription pipeline.</p>
        </div>

        <div class="setting-row">
          <div class="shortcut-meta">
            <span class="shortcut-label">Verbose transcription logs</span>
            <span class="shortcut-help">Includes per-frame STT logging and debug audio capture.</span>
          </div>
          <input
            type="checkbox"
            checked={app.settings.debug_transcription}
            onchange={onDebugTranscriptionChange}
            class="switch"
            aria-label="Verbose transcription debug logs"
          />
        </div>

        <p class="helper-text">Leave this off during normal use so the app stays quiet and lightweight.</p>
      </div>
    </section>

    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">Ollama</p>
          <h3 class="section-title">Local summary defaults</h3>
          <p class="section-description">Use a text-generation model for meeting summaries while keeping the connection state visible.</p>
        </div>

        <label class="field-group">
          <span class="field-label">Ollama URL</span>
          <input type="text" value={app.settings.ollama_url} onchange={onOllamaUrlChange} class="field-input" />
        </label>

        <div class="setting-row">
          <div class="shortcut-meta">
            <span class="shortcut-label">Connection status</span>
            <span class="shortcut-help">Checks the configured Ollama endpoint for compatible summary models.</span>
          </div>
          <div class="action-row" style="flex-wrap: wrap; justify-content: flex-end;">
            <div class="action-row">
              <span class="status-dot" class:is-online={ollamaAvailable}></span>
              <span class="helper-text">{ollamaAvailable ? "Connected" : "Not available"}</span>
            </div>
            <button onclick={checkOllama} class="button button-secondary">Retry</button>
          </div>
        </div>

        {#if ollamaAvailable && ollamaSummaryModels.length > 0}
          <label class="field-group">
            <span class="field-label">Default summary model</span>
            <select
              value={app.settings.ollama_model || ollamaSummaryModels[0]}
              onchange={onOllamaModelChange}
              class="field-select"
            >
              {#each ollamaSummaryModels as model}
                <option value={model}>{model}</option>
              {/each}
            </select>
          </label>
          <p class="helper-text">Speech and embedding models stay hidden here so the chooser remains focused on summary-capable options.</p>
        {:else if ollamaAvailable}
          <div class="status-banner status-banner--warning">
            <strong>No summary-capable model found.</strong>
            <p class="helper-text">Install a chat or instruction model to enable reliable summaries.</p>
          </div>
        {/if}
      </div>
    </section>

    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">Appearance</p>
          <h3 class="section-title">Theme</h3>
          <p class="section-description">The refreshed interface supports dark, light, and system-driven presentation without losing contrast.</p>
        </div>

        <div class="theme-group">
          {#each ["dark", "light", "system"] as themeOption}
            <button
              onclick={() => onThemeChange(themeOption as Theme)}
              class="theme-button"
              class:is-active={app.settings.theme === themeOption}
            >
              {themeOption.charAt(0).toUpperCase() + themeOption.slice(1)}
            </button>
          {/each}
        </div>
      </div>
    </section>

    <section class="settings-card">
      <div class="settings-card-body stack-md">
        <div>
          <p class="eyebrow">About</p>
          <h3 class="section-title">Soufflé</h3>
          <p class="section-description">Local speech-to-text for macOS with a minimal UI shell and local model execution.</p>
        </div>

        <div class="settings-list">
          <div class="setting-row">
            <span class="shortcut-label">Version</span>
            <span class="helper-text">v0.1.0</span>
          </div>
          <div class="setting-row">
            <span class="shortcut-label">Engine</span>
            <span class="helper-text">Kyutai STT 1B (FR/EN)</span>
          </div>
          <div class="setting-row">
            <span class="shortcut-label">Privacy</span>
            <span class="helper-text">Everything runs locally.</span>
          </div>
        </div>
      </div>
    </section>
  </div>
</div>
