<script lang="ts">
  import type { View } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  const tabs: { id: View; label: string; icon: string }[] = [
    { id: "dictation", label: "Dictation", icon: "mic" },
    { id: "recordings", label: "Recordings", icon: "list" },
    { id: "settings", label: "Settings", icon: "gear" },
  ];
</script>

<nav class="flex border-b border-zinc-800 dark:border-zinc-800 light:border-zinc-200 w-full">
  {#each tabs as tab}
    <button
      onclick={() => app.currentView = tab.id}
      class="flex-1 px-4 py-2.5 text-xs font-medium transition-colors cursor-pointer
        {app.currentView === tab.id
          ? 'text-zinc-100 border-b-2 border-blue-500 dark:text-zinc-100'
          : 'text-zinc-500 hover:text-zinc-300 dark:text-zinc-500 dark:hover:text-zinc-300'}"
    >
      {tab.label}
    </button>
  {/each}
</nav>
