<script lang="ts">
  import type { View } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  const tabs: { id: View; label: string; caption: string }[] = [
    { id: "dictation", label: "Dictation", caption: "Quick voice capture" },
    { id: "recordings", label: "Recordings", caption: "Meetings and summaries" },
    { id: "settings", label: "Settings", caption: "Devices and automation" },
  ];
</script>

<nav class="tab-nav" aria-label="Primary navigation">
  {#each tabs as tab}
    <button
      onclick={() => app.currentView = tab.id}
      class="tab-button"
      class:is-active={app.currentView === tab.id}
      aria-current={app.currentView === tab.id ? "page" : undefined}
    >
      <span class="tab-label">{tab.label}</span>
      <span class="tab-caption">{tab.caption}</span>
    </button>
  {/each}
</nav>
