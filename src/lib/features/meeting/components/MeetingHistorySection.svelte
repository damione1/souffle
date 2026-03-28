<script lang="ts">
  import { ChevronRight } from "@lucide/svelte";
  import { t } from "svelte-i18n";
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
    <h2>{$t("meeting_history.title")}</h2>
    <span class="pill">{$t("meeting_history.meetings_count", { values: { count: meetings.length } })}</span>
  </div>

  {#if statusMessage}
    <StatusBanner message={statusMessage} />
  {/if}

  <input
    type="text"
    bind:value={searchQuery}
    placeholder={$t("meeting_history.search_placeholder")}
    class="field-input"
  />

  {#if filteredMeetings.length === 0}
    <EmptyState
      title={searchQuery ? $t("meeting_history.no_matches_title") : $t("meeting_history.no_meetings_title")}
      message={searchQuery ? $t("meeting_history.no_matches_msg") : $t("meeting_history.no_meetings_msg")}
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
                <span class={`pill ${meeting.summary_is_stale ? "pill-warning" : "pill-success"}`}>
                  {meeting.summary_is_stale ? $t("meeting_history.summary_outdated") : $t("meeting_history.summary")}
                </span>
              {/if}
            </div>
            <ChevronRight size={16} class="text-text-muted shrink-0" />
          </div>
        </button>
      {/each}
    </div>
  {/if}
</div>
