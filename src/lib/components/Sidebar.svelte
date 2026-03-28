<script lang="ts">
  import { History, Mic, Settings, Users, Radio } from "@lucide/svelte";
  import type { Component } from "svelte";
  import type { AppView } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  const tabs: { id: AppView; label: string; icon: Component }[] = [
    {
      id: "transcription",
      label: "Transcription",
      icon: Mic,
    },
    {
      id: "meeting",
      label: "Meeting",
      icon: Users,
    },
    {
      id: "meeting-history",
      label: "History",
      icon: History,
    },
    {
      id: "settings",
      label: "Settings",
      icon: Settings,
    },
  ];

  function recordingTargetView(): AppView {
    return app.recordingMode === "meeting" ? "meeting" : "transcription";
  }
</script>

<aside class="w-[200px] min-w-[200px] h-screen flex flex-col gap-6 py-5 px-3 bg-surface-1 border-r border-ghost-border overflow-y-auto max-[800px]:w-[72px] max-[800px]:min-w-[72px] max-[800px]:items-center">
  <div class="flex items-center gap-2.5 px-2 max-[800px]:justify-center max-[800px]:px-0">
    <span class="flex items-center justify-center w-8 h-8 rounded-lg bg-accent-blue text-white font-heading font-extrabold text-base shrink-0">S</span>
    <span class="font-heading font-bold text-lg text-text-primary tracking-tight max-[800px]:hidden">Soufflé</span>
  </div>

  <nav class="flex flex-col gap-1" aria-label="Primary navigation">
    {#each tabs as tab}
      {@const isActive = app.currentView === tab.id}
      {@const Icon = tab.icon}
      <button
        onclick={() => (app.currentView = tab.id)}
        class={`relative flex items-center gap-2.5 py-2.5 px-3 rounded-default cursor-pointer transition-[background,color] duration-150 max-[800px]:justify-center max-[800px]:p-3 ${
          isActive
            ? "bg-accent-blue/10 text-accent-blue"
            : "text-text-muted hover:bg-surface-2 hover:text-text-secondary"
        }`}
        aria-current={isActive ? "page" : undefined}
        aria-label={tab.label}
        >
          <span
            class={`absolute left-0 top-1/2 -translate-y-1/2 w-[3px] rounded-r-sm bg-accent-blue transition-[height] duration-150 ${isActive ? "h-5" : "h-0"}`}
            aria-hidden="true"
          ></span>
        <Icon size={20} strokeWidth={1.75} aria-hidden="true" />
        <span class="text-sm font-medium max-[800px]:hidden">{tab.label}</span>
      </button>
    {/each}
  </nav>

  {#if app.isRecording}
    <button
      onclick={() => (app.currentView = recordingTargetView())}
      class="flex items-center gap-2 mx-2 mt-auto mb-2 px-3 py-2 rounded-default bg-red-500/15 text-red-400 hover:bg-red-500/25 transition-colors cursor-pointer"
      aria-label="Go to active recording"
    >
      <Radio size={16} strokeWidth={2} class="animate-pulse" aria-hidden="true" />
      <span class="text-xs font-medium max-[800px]:hidden">
        {app.recordingMode === "meeting" ? "Meeting" : "Dictation"}
      </span>
    </button>
  {/if}
</aside>
