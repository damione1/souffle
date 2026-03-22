<script lang="ts">
  import type { View } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  const tabs: { id: View; label: string; icon: string }[] = [
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

<aside class="sidebar">
  <div class="sidebar-brand">
    <span class="sidebar-logo">S</span>
    <span class="sidebar-title">Soufflé</span>
  </div>

  <nav class="sidebar-nav" aria-label="Primary navigation">
    {#each tabs as tab}
      <button
        onclick={() => (app.currentView = tab.id)}
        class="sidebar-item"
        class:is-active={app.currentView === tab.id}
        aria-current={app.currentView === tab.id ? "page" : undefined}
      >
        <span class="sidebar-indicator" aria-hidden="true"></span>
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
        <span class="sidebar-label">{tab.label}</span>
      </button>
    {/each}
  </nav>
</aside>

<style>
  .sidebar {
    width: 200px;
    min-width: 200px;
    height: 100vh;
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    padding: 1.25rem 0.75rem;
    background: var(--color-surface-1);
    border-right: 1px solid var(--color-ghost-border);
    overflow-y: auto;
  }

  .sidebar-brand {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    padding: 0 0.5rem;
  }

  .sidebar-logo {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    background: var(--color-accent-blue);
    color: #fff;
    font-family: var(--font-family-heading);
    font-weight: 800;
    font-size: 1rem;
    flex-shrink: 0;
  }

  .sidebar-title {
    font-family: var(--font-family-heading);
    font-weight: 700;
    font-size: 1.125rem;
    color: var(--color-text-primary);
    letter-spacing: -0.02em;
  }

  .sidebar-nav {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .sidebar-item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 0.625rem;
    padding: 0.625rem 0.75rem;
    border-radius: var(--radius-default);
    color: var(--color-text-muted);
    cursor: pointer;
    transition: background 150ms ease, color 150ms ease;
  }

  .sidebar-item:hover {
    background: var(--color-surface-2);
    color: var(--color-text-secondary);
  }

  .sidebar-item.is-active {
    background: color-mix(in srgb, var(--color-accent-blue) 10%, transparent);
    color: var(--color-accent-blue);
  }

  .sidebar-indicator {
    position: absolute;
    left: 0;
    top: 50%;
    transform: translateY(-50%);
    width: 3px;
    height: 0;
    border-radius: 0 2px 2px 0;
    background: var(--color-accent-blue);
    transition: height 150ms ease;
  }

  .sidebar-item.is-active .sidebar-indicator {
    height: 1.25rem;
  }

  .sidebar-label {
    font-size: 0.875rem;
    font-weight: 500;
  }

  @media (max-width: 800px) {
    .sidebar {
      width: 72px;
      min-width: 72px;
      align-items: center;
    }

    .sidebar-title,
    .sidebar-label {
      display: none;
    }

    .sidebar-brand {
      justify-content: center;
      padding: 0;
    }

    .sidebar-item {
      justify-content: center;
      padding: 0.75rem;
    }
  }
</style>
