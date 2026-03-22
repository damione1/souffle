<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type { MeetingListItem } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  let meetings = $state<MeetingListItem[]>([]);
  let statusMessage = $state("");
  let searchQuery = $state("");

  let filteredMeetings = $derived(
    searchQuery.trim()
      ? meetings.filter((m) => m.title.toLowerCase().includes(searchQuery.toLowerCase()))
      : meetings
  );

  onMount(async () => {
    await refreshMeetings();
  });

  async function refreshMeetings() {
    try {
      meetings = await invoke("list_meetings");
    } catch (e) {
      statusMessage = String(e);
    }
  }

  function formatDuration(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  }

  function formatDate(iso: string): string {
    return new Date(iso).toLocaleString();
  }
</script>

<div class="view">
  <div class="header-row">
    <h2>Meeting History</h2>
    <span class="pill">{meetings.length} meetings</span>
  </div>

  {#if statusMessage}
    <div class="status-banner">
      <p class="text-sm">{statusMessage}</p>
    </div>
  {/if}

  <input
    type="text"
    bind:value={searchQuery}
    placeholder="Search meetings..."
    class="field-input"
  />

  {#if filteredMeetings.length === 0}
    <div class="empty-state">
      <strong>{searchQuery ? "No matches" : "No meetings yet"}</strong>
      <p class="text-sm text-muted">
        {searchQuery ? "Try a different search term." : "Start a meeting recording to build your transcript library."}
      </p>
    </div>
  {:else}
    <div class="meeting-list">
      {#each filteredMeetings as meeting}
        <button
          onclick={() => app.openMeeting(meeting.id)}
          class="meeting-card"
        >
          <div class="meeting-info">
            <span class="meeting-title">{meeting.title}</span>
            <span class="text-sm text-muted">{formatDate(meeting.started_at)}</span>
          </div>
          <div class="meeting-right">
            <div class="meeting-badges">
              <span class="pill">{formatDuration(meeting.duration_seconds)}</span>
              {#if meeting.has_summary}
                <span class="pill pill-success">Summary</span>
              {/if}
            </div>
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16" class="chevron">
              <path fill-rule="evenodd" d="M8.22 5.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L11.94 10 8.22 6.28a.75.75 0 0 1 0-1.06Z" clip-rule="evenodd" />
            </svg>
          </div>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .header-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .empty-state {
    padding: 2rem;
    text-align: center;
    color: var(--color-text-muted);
  }

  .empty-state strong {
    display: block;
    margin-bottom: 0.25rem;
    color: var(--color-text-secondary);
  }

  .status-banner {
    padding: 0.75rem 1rem;
    border-radius: var(--radius-default);
    background: var(--color-surface-3);
    outline: 1px solid var(--color-ghost-border);
  }

  .text-sm {
    font-size: 0.8125rem;
  }

  .text-muted {
    color: var(--color-text-muted);
  }

  .meeting-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .meeting-card {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.875rem 1rem;
    border-radius: var(--radius-default);
    outline: 1px solid var(--color-ghost-border);
    background: var(--color-surface-2);
    text-align: left;
    cursor: pointer;
    transition: background 150ms ease, outline-color 150ms ease;
  }

  .meeting-card:hover {
    background: var(--color-surface-3);
    outline-color: color-mix(in srgb, var(--color-accent-blue) 30%, var(--color-ghost-border));
  }

  .meeting-info {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    min-width: 0;
  }

  .meeting-title {
    font-weight: 600;
    color: var(--color-text-primary);
    font-size: 0.9375rem;
  }

  .meeting-right {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-shrink: 0;
  }

  .meeting-badges {
    display: flex;
    gap: 0.375rem;
  }

  .chevron {
    color: var(--color-text-muted);
    flex-shrink: 0;
  }
</style>
