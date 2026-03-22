<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import Dictation from "./lib/components/Dictation.svelte";
  import Recordings from "./lib/components/Recordings.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import { getAppState } from "./lib/stores/app.svelte";
  import type { Theme, View } from "./lib/types";

  const app = getAppState();

  let unlistenNav: (() => void) | null = null;

  onMount(() => {
    // Load saved settings from SQLite
    (async () => {
      try {
        const settings = await invoke<Record<string, unknown>>("get_settings");
        if (settings.theme) {
          const theme = settings.theme as Theme;
          app.theme = theme;
          app.settings = { ...app.settings, theme };
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
        if (settings.audio_device) {
          app.selectedDevice = settings.audio_device as string;
          await invoke("select_audio_device", { deviceName: app.selectedDevice });
        }
      } catch {
        // First run, no settings yet.
      }
    })();

    listen<string>("navigate", (event) => {
      const view = event.payload as View;
      if (["dictation", "recordings", "settings"].includes(view)) {
        app.currentView = view;
      }
    }).then((fn) => {
      unlistenNav = fn;
    });

    return () => {
      unlistenNav?.();
    };
  });

  function applyTheme(theme: Theme) {
    if (theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)) {
      document.documentElement.classList.add("dark");
      document.documentElement.classList.remove("light");
    } else if (theme === "light") {
      document.documentElement.classList.remove("dark");
      document.documentElement.classList.add("light");
    } else {
      const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      document.documentElement.classList.toggle("dark", isDark);
      document.documentElement.classList.toggle("light", !isDark);
    }
  }
</script>

<main class="app-shell">
  <div class="app-frame">
    <TabBar />

    <section class="page-surface">
      {#if app.currentView === "dictation"}
        <Dictation />
      {:else if app.currentView === "recordings"}
        <Recordings />
      {:else if app.currentView === "settings"}
        <Settings />
      {/if}
    </section>
  </div>
</main>
