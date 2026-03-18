<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { load } from "@tauri-apps/plugin-store";
  import TabBar from "./lib/components/TabBar.svelte";
  import Dictation from "./lib/components/Dictation.svelte";
  import Recordings from "./lib/components/Recordings.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import { getAppState } from "./lib/stores/app.svelte";
  import type { Theme, View } from "./lib/types";

  const app = getAppState();

  let unlistenNav: (() => void) | null = null;

  onMount(() => {
    // Load saved settings
    (async () => {
      try {
        const store = await load("settings.json", { defaults: {}, autoSave: true });
        const theme = await store.get<Theme>("theme");
        if (theme) {
          app.theme = theme;
          app.settings = { ...app.settings, theme };
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
      } catch { /* First run, no settings yet */ }
    })();

    // Listen for tray navigation events
    listen<string>("navigate", (event) => {
      const view = event.payload as View;
      if (["dictation", "recordings", "settings"].includes(view)) {
        app.currentView = view;
      }
    }).then((fn) => { unlistenNav = fn; });

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
      // system
      const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      document.documentElement.classList.toggle("dark", isDark);
      document.documentElement.classList.toggle("light", !isDark);
    }
  }
</script>

<main class="flex flex-col items-center min-h-screen">
  <TabBar />
  <div class="flex flex-col items-center justify-center flex-1 p-6 w-full">
    {#if app.currentView === "dictation"}
      <Dictation />
    {:else if app.currentView === "recordings"}
      <Recordings />
    {:else if app.currentView === "settings"}
      <Settings />
    {/if}
  </div>
</main>
