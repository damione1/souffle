<script lang="ts">
  import type { AppView } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  const tabs: { id: AppView; label: string; icon: string }[] = [
    {
      id: "transcription",
      label: "Transcription",
      icon: "M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4ZM6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z",
    },
    {
      id: "meeting",
      label: "Meeting",
      icon: "M15 8a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM15 10c-3.87 0-7 2.69-7 6v1a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-1c0-3.31-3.13-6-7-6ZM6 8a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM6 10c-2.67 0-5 1.79-5 4v1a1 1 0 0 0 1 1h4",
    },
    {
      id: "meeting-history",
      label: "History",
      icon: "M12 8v4l3 3m6-3a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z",
    },
    {
      id: "settings",
      label: "Settings",
      icon: "M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.325.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 0 1 1.37.49l1.296 2.247a1.125 1.125 0 0 1-.26 1.431l-1.003.827c-.293.24-.438.613-.431.992a7 7 0 0 1 0 .255c-.007.378.138.75.43.99l1.005.828c.424.35.534.954.26 1.43l-1.298 2.247a1.125 1.125 0 0 1-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a7 7 0 0 1-.22.128c-.331.183-.581.495-.644.869l-.213 1.281c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.019-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a7 7 0 0 1-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 0 1-1.369-.49l-1.297-2.247a1.125 1.125 0 0 1 .26-1.431l1.004-.827c.292-.24.437-.613.43-.991a7 7 0 0 1 0-.255c.007-.38-.138-.751-.43-.992l-1.004-.827a1.125 1.125 0 0 1-.26-1.43l1.297-2.247a1.125 1.125 0 0 1 1.37-.491l1.216.456c.356.133.751.072 1.076-.124q.108-.066.22-.128c.332-.183.582-.495.644-.869zM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z",
    },
  ];
</script>

<aside class="w-[200px] min-w-[200px] h-screen flex flex-col gap-6 py-5 px-3 bg-surface-1 border-r border-ghost-border overflow-y-auto max-[800px]:w-[72px] max-[800px]:min-w-[72px] max-[800px]:items-center">
  <div class="flex items-center gap-2.5 px-2 max-[800px]:justify-center max-[800px]:px-0">
    <span class="flex items-center justify-center w-8 h-8 rounded-lg bg-accent-blue text-white font-heading font-extrabold text-base shrink-0">S</span>
    <span class="font-heading font-bold text-lg text-text-primary tracking-tight max-[800px]:hidden">Soufflé</span>
  </div>

  <nav class="flex flex-col gap-1" aria-label="Primary navigation">
    {#each tabs as tab}
      {@const isActive = app.currentView === tab.id}
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
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          width="20"
          height="20"
          aria-hidden="true"
        >
          <path d={tab.icon} />
        </svg>
        <span class="text-sm font-medium max-[800px]:hidden">{tab.label}</span>
      </button>
    {/each}
  </nav>
</aside>
