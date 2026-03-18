<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { load } from "@tauri-apps/plugin-store";
  import { onMount } from "svelte";
  import type { AudioDevice, OllamaStatus, Theme } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  let audioDevices = $state<AudioDevice[]>([]);
  let selectedDevice = $state("");
  let ollamaAvailable = $state(false);
  let ollamaModels = $state<string[]>([]);
  let statusMessage = $state("");

  onMount(async () => {
    await loadSettings();
    await refreshDevices();
    await checkOllama();
  });

  async function loadSettings() {
    try {
      const store = await load("settings.json", { defaults: {}, autoSave: true });
      const theme = await store.get<Theme>("theme");
      if (theme) {
        app.settings = { ...app.settings, theme };
        app.theme = theme;
        applyTheme(theme);
      }
      const autoPaste = await store.get<boolean>("auto_paste");
      if (autoPaste !== null && autoPaste !== undefined) {
        app.settings = { ...app.settings, auto_paste: autoPaste };
      }
      const pasteDelay = await store.get<number>("paste_delay_ms");
      if (pasteDelay !== null && pasteDelay !== undefined) {
        app.settings = { ...app.settings, paste_delay_ms: pasteDelay };
      }
      const ollamaUrl = await store.get<string>("ollama_url");
      if (ollamaUrl) {
        app.settings = { ...app.settings, ollama_url: ollamaUrl };
      }
      const ollamaModel = await store.get<string>("ollama_model");
      if (ollamaModel) {
        app.settings = { ...app.settings, ollama_model: ollamaModel };
      }
    } catch (e) {
      console.warn("Failed to load settings:", e);
    }
  }

  async function saveSetting(key: string, value: unknown) {
    try {
      const store = await load("settings.json", { defaults: {}, autoSave: true });
      await store.set(key, value);
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function refreshDevices() {
    try {
      audioDevices = await invoke("list_audio_devices");
      const defaultDev = audioDevices.find((d) => d.is_default);
      if (!selectedDevice && defaultDev) {
        selectedDevice = defaultDev.name;
      }
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function onDeviceChange(event: Event) {
    const target = event.target as HTMLSelectElement;
    selectedDevice = target.value;
    try {
      await invoke("select_audio_device", { deviceName: selectedDevice });
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function checkOllama() {
    try {
      const status: OllamaStatus = await invoke("check_ollama");
      ollamaAvailable = status.available;
      ollamaModels = status.models;
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
</script>

<div class="flex flex-col gap-6 w-full max-w-lg">
  <h2 class="text-lg font-semibold text-zinc-100">Settings</h2>

  {#if statusMessage}
    <p class="text-xs text-yellow-500">{statusMessage}</p>
  {/if}

  <!-- Audio -->
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-medium text-zinc-300">Audio</h3>
    <div class="flex flex-col gap-2 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
      <span class="text-xs text-zinc-400">Input Device</span>
      <div class="flex items-center gap-2">
        <select
          value={selectedDevice}
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
      {#if ollamaAvailable && ollamaModels.length > 0}
        <label class="flex flex-col gap-1">
          <span class="text-xs text-zinc-400">Default Model</span>
          <select
            value={app.settings.ollama_model || ollamaModels[0]}
            onchange={onOllamaModelChange}
            class="px-3 py-1.5 text-xs rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300
              focus:border-zinc-500 focus:outline-none"
          >
            {#each ollamaModels as model}
              <option value={model}>{model}</option>
            {/each}
          </select>
        </label>
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
