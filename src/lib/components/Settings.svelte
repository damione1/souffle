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

  // Shortcut state
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
      // If we have a saved device and it exists in the list, use it
      // Otherwise fall back to the system default
      if (app.selectedDevice) {
        const exists = audioDevices.some((d) => d.name === app.selectedDevice);
        if (exists) {
          // Re-send to backend in case it wasn't set yet
          await invoke("select_audio_device", { deviceName: app.selectedDevice });
          return;
        }
      }
      const defaultDev = audioDevices.find((d) => d.is_default);
      if (defaultDev) {
        app.selectedDevice = defaultDev.name;
        await invoke("select_audio_device", { deviceName: defaultDev.name });
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

  // ─── Shortcut recording ─────────────────────────────────────────

  /** Convert a KeyboardEvent into the Tauri shortcut string format */
  function keyEventToShortcut(e: KeyboardEvent): string | null {
    // Ignore standalone modifier presses
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return null;

    const parts: string[] = [];
    if (e.metaKey || e.ctrlKey) parts.push("CommandOrControl");
    if (e.shiftKey) parts.push("Shift");
    if (e.altKey) parts.push("Alt");

    // Map key to Tauri Code name
    const key = mapKey(e.code, e.key);
    if (!key) return null;
    parts.push(key);

    return parts.join("+");
  }

  function mapKey(code: string, key: string): string | null {
    // Function keys
    if (/^F\d{1,2}$/.test(key)) return key;

    // Letter/digit keys from code (e.g., "KeyA" → "A", "Digit1" → "1")
    if (code.startsWith("Key")) return code.slice(3);
    if (code.startsWith("Digit")) return code.slice(5);

    // Special keys
    const map: Record<string, string> = {
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

    return map[code] || null;
  }

  /** Format a shortcut string for display (replace Tauri syntax with symbols) */
  function formatShortcut(s: string): string {
    if (!s) return "Not set";
    return s
      .replace(/CommandOrControl/g, "⌘")
      .replace(/Shift/g, "⇧")
      .replace(/Alt/g, "⌥")
      .replace(/\+/g, " ");
  }

  function startRecording(field: "toggle" | "ptt") {
    recordingField = field;
    shortcutError = "";
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (!recordingField) return;
    e.preventDefault();
    e.stopPropagation();

    // Escape cancels recording
    if (e.key === "Escape") {
      recordingField = null;
      return;
    }

    // Backspace/Delete clears the shortcut
    if (e.key === "Backspace" || e.key === "Delete") {
      if (recordingField === "toggle") toggleShortcut = "";
      else pttShortcut = "";
      recordingField = null;
      saveShortcuts();
      return;
    }

    const shortcut = keyEventToShortcut(e);
    if (!shortcut) return; // Still pressing modifiers only

    // Must have at least one modifier
    if (!e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey && !/^F\d{1,2}$/.test(e.key)) {
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

<div class="flex flex-col gap-6 w-full max-w-lg">
  <h2 class="text-lg font-semibold text-zinc-100">Settings</h2>

  {#if statusMessage}
    <p class="text-xs text-yellow-500">{statusMessage}</p>
  {/if}

  <!-- Keyboard Shortcuts -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Keyboard Shortcuts</h3>
    <div class="flex flex-col gap-3 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <!-- Toggle recording -->
      <div class="flex items-center justify-between">
        <div class="flex flex-col gap-0.5">
          <span class="text-xs text-zinc-400">Toggle recording</span>
          <span class="text-[10px] text-zinc-600">Press to start/stop</span>
        </div>
        <div class="flex items-center gap-2">
          <button
            onclick={() => startRecording("toggle")}
            class="px-3 py-1.5 text-xs rounded-lg transition-colors cursor-pointer
              {recordingField === 'toggle'
                ? 'bg-blue-600 text-white animate-pulse'
                : 'bg-zinc-800 border border-zinc-700 text-zinc-300 hover:border-zinc-500'}"
          >
            {recordingField === "toggle" ? "Press keys..." : formatShortcut(toggleShortcut)}
          </button>
          {#if toggleShortcut}
            <button
              onclick={() => clearShortcut("toggle")}
              class="text-xs text-zinc-600 hover:text-zinc-400 cursor-pointer"
              title="Clear shortcut"
            >
              ×
            </button>
          {/if}
        </div>
      </div>

      <!-- Push-to-talk -->
      <div class="flex items-center justify-between">
        <div class="flex flex-col gap-0.5">
          <span class="text-xs text-zinc-400">Push-to-talk</span>
          <span class="text-[10px] text-zinc-600">Hold to record, release to stop</span>
        </div>
        <div class="flex items-center gap-2">
          <button
            onclick={() => startRecording("ptt")}
            class="px-3 py-1.5 text-xs rounded-lg transition-colors cursor-pointer
              {recordingField === 'ptt'
                ? 'bg-blue-600 text-white animate-pulse'
                : 'bg-zinc-800 border border-zinc-700 text-zinc-300 hover:border-zinc-500'}"
          >
            {recordingField === "ptt" ? "Press keys..." : formatShortcut(pttShortcut)}
          </button>
          {#if pttShortcut}
            <button
              onclick={() => clearShortcut("ptt")}
              class="text-xs text-zinc-600 hover:text-zinc-400 cursor-pointer"
              title="Clear shortcut"
            >
              ×
            </button>
          {/if}
        </div>
      </div>

      {#if shortcutError}
        <p class="text-xs text-red-400">{shortcutError}</p>
      {/if}

      <p class="text-xs text-zinc-600">
        Click a shortcut, then press the desired key combination. Press Escape to cancel, Delete to clear.
      </p>
    </div>
  </section>

  <!-- Audio -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Audio</h3>
    <div class="flex flex-col gap-2 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <span class="text-xs text-zinc-400">Input Device</span>
      <div class="flex items-center gap-2">
        <select
          value={app.selectedDevice}
          onchange={onDeviceChange}
          class="flex-1 px-3 py-1.5 text-xs rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300
            focus:border-zinc-500 focus:outline-none"
        >
          {#each audioDevices as device}
            <option value={device.name}>
              {device.name}{device.is_default ? " (default)" : ""}
            </option>
          {/each}
        </select>
        <button
          onclick={refreshDevices}
          class="px-2 py-1.5 text-xs rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-400
            hover:text-zinc-200 hover:border-zinc-500 cursor-pointer transition-colors"
        >
          ↻
        </button>
      </div>
    </div>
  </section>

  <!-- Dictation -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Dictation</h3>
    <div class="flex flex-col gap-3 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <label class="flex items-center justify-between">
        <span class="text-xs text-zinc-400">Auto-paste after dictation</span>
        <input
          type="checkbox"
          checked={app.settings.auto_paste}
          onchange={onAutoPasteChange}
          class="w-4 h-4 rounded bg-zinc-700 border-zinc-600 cursor-pointer"
        />
      </label>
      <label class="flex items-center justify-between">
        <span class="text-xs text-zinc-400">Paste delay (ms)</span>
        <input
          type="number"
          value={app.settings.paste_delay_ms}
          onchange={onPasteDelayChange}
          min="50"
          max="1000"
          step="50"
          class="w-20 px-2 py-1 text-xs rounded bg-zinc-800 border border-zinc-700 text-zinc-300 text-right
            focus:border-zinc-500 focus:outline-none"
        />
      </label>
      <p class="text-xs text-zinc-600">
        Auto-paste copies transcribed text and simulates Cmd+V. Requires Accessibility permission.
      </p>
    </div>
  </section>

  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Diagnostics</h3>
    <div class="flex flex-col gap-3 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <label class="flex items-center justify-between">
        <span class="text-xs text-zinc-400">Verbose transcription debug logs</span>
        <input
          type="checkbox"
          checked={app.settings.debug_transcription}
          onchange={onDebugTranscriptionChange}
          class="w-4 h-4 rounded bg-zinc-700 border-zinc-600 cursor-pointer"
        />
      </label>
      <p class="text-xs text-zinc-600">
        Enables per-frame STT logging and debug audio capture for troubleshooting. Leave this off during normal use.
      </p>
    </div>
  </section>

  <!-- Ollama -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Ollama</h3>
    <div class="flex flex-col gap-3 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <label class="flex flex-col gap-1">
        <span class="text-xs text-zinc-400">Ollama URL</span>
        <input
          type="text"
          value={app.settings.ollama_url}
          onchange={onOllamaUrlChange}
          class="px-3 py-1.5 text-xs rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300
            focus:border-zinc-500 focus:outline-none"
        />
      </label>
      <div class="flex items-center justify-between">
        <span class="text-xs text-zinc-400">Status</span>
        <div class="flex items-center gap-2">
          <div class="w-2 h-2 rounded-full {ollamaAvailable ? 'bg-green-500' : 'bg-red-500'}"></div>
          <span class="text-xs {ollamaAvailable ? 'text-green-400' : 'text-red-400'}">
            {ollamaAvailable ? "Connected" : "Not available"}
          </span>
          <button
            onclick={checkOllama}
            class="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer"
          >
            Retry
          </button>
        </div>
      </div>
      {#if ollamaAvailable && ollamaSummaryModels.length > 0}
        <label class="flex flex-col gap-1">
          <span class="text-xs text-zinc-400">Default Summary Model</span>
          <select
            value={app.settings.ollama_model || ollamaSummaryModels[0]}
            onchange={onOllamaModelChange}
            class="px-3 py-1.5 text-xs rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300
              focus:border-zinc-500 focus:outline-none"
          >
            {#each ollamaSummaryModels as model}
              <option value={model}>{model}</option>
            {/each}
          </select>
        </label>
        <p class="text-xs text-zinc-500">
          Speech and embedding models are hidden here. Meeting summaries work best with chat or instruction models.
        </p>
      {:else if ollamaAvailable}
        <p class="text-xs text-amber-400">
          No summary-capable Ollama model detected. Install a text-generation model to enable reliable meeting summaries.
        </p>
      {/if}
    </div>
  </section>

  <!-- Theme -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Appearance</h3>
    <div class="flex flex-col gap-2 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <span class="text-xs text-zinc-400">Theme</span>
      <div class="flex gap-2">
        {#each ["dark", "light", "system"] as t}
          <button
            onclick={() => onThemeChange(t as Theme)}
            class="flex-1 px-3 py-1.5 text-xs rounded-lg cursor-pointer transition-colors
              {app.settings.theme === t
                ? 'bg-blue-600 text-white'
                : 'bg-zinc-800 border border-zinc-700 text-zinc-400 hover:text-zinc-200'}"
          >
            {t.charAt(0).toUpperCase() + t.slice(1)}
          </button>
        {/each}
      </div>
    </div>
  </section>

  <!-- About -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">About</h3>
    <div class="flex flex-col gap-1 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <p class="text-xs text-zinc-400">Souffle v0.1.0</p>
      <p class="text-xs text-zinc-500">Local speech-to-text desktop application</p>
      <p class="text-xs text-zinc-500">Engine: Kyutai STT 1B (FR/EN)</p>
      <p class="text-xs text-zinc-600 mt-1">Privacy-first. Everything runs locally.</p>
    </div>
  </section>
</div>
