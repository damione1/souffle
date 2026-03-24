<script lang="ts">
  import type { MeetingListItem } from "../../../types";
  import { formatDate, formatDuration } from "../../../utils";
  import EmptyState from "../../../components/ui/EmptyState.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";

  let {
    meetings,
    filteredMeetings,
    statusMessage,
    searchQuery = $bindable(),
    onOpenMeeting,
  }: {
    meetings: MeetingListItem[];
    filteredMeetings: MeetingListItem[];
    statusMessage: string;
    searchQuery: string;
    onOpenMeeting: (id: string) => void;
  } = $props();
</script>

<div class="flex flex-col gap-4">
  <div class="flex items-center gap-3">
    <h2>Meeting History</h2>
    <span class="pill">{meetings.length} meetings</span>
  </div>

  {#if statusMessage}
    <StatusBanner message={statusMessage} />
  {/if}

  <input
    type="text"
    bind:value={searchQuery}
    placeholder="Search meetings..."
    class="field-input"
  />

  {#if filteredMeetings.length === 0}
    <EmptyState
      title={searchQuery ? "No matches" : "No meetings yet"}
      message={searchQuery ? "Try a different search term." : "Start a meeting recording to build your transcript library."}
    />
  {:else}
    <div class="flex flex-col gap-2">
      {#each filteredMeetings as meeting}
        <button
          onclick={() => onOpenMeeting(meeting.id)}
          class="w-full flex items-center justify-between gap-4 px-4 py-3.5 rounded-default outline-1 outline-ghost-border bg-surface-2 text-left cursor-pointer transition-[background,outline-color] duration-150 hover:bg-surface-3 hover:outline-accent-blue/30"
        >
          <div class="flex flex-col gap-0.5 min-w-0">
            <span class="font-semibold text-text-primary text-[0.9375rem]">{meeting.title}</span>
            <span class="text-sm text-text-muted">{formatDate(meeting.started_at)}</span>
          </div>
          <div class="flex items-center gap-3 shrink-0">
            <div class="flex gap-1.5">
              <span class="pill">{formatDuration(meeting.duration_seconds)}</span>
              {#if meeting.has_summary}
                <span class="pill pill-success">Summary</span>
              {/if}
            </div>
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16" class="text-text-muted shrink-0">
              <path fill-rule="evenodd" d="M8.22 5.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L11.94 10 8.22 6.28a.75.75 0 0 1 0-1.06Z" clip-rule="evenodd" />
            </svg>
          </div>
        </button>
      {/each}
    </div>
  {/if}
</div>
