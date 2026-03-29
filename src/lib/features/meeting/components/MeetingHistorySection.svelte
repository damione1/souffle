<script lang="ts">
  import { ChevronRight, FileText, Mic } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { MeetingListItem, SearchResult } from "../../../types";
  import { filterResultsByType, findSnippet, formatDate, formatDuration } from "../../../utils";
  import EmptyState from "../../../components/ui/EmptyState.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";

  let {
    meetings,
    filteredMeetings,
    statusMessage,
    searchQuery = $bindable(),
    searchResults,
    isSearching,
    onOpenMeeting,
  }: {
    meetings: MeetingListItem[];
    filteredMeetings: MeetingListItem[];
    statusMessage: string;
    searchQuery: string;
    searchResults: SearchResult[];
    isSearching: boolean;
    onOpenMeeting: (id: string) => void;
  } = $props();

  let hasSearchQuery = $derived(searchQuery.trim().length > 0);
  let dictationResults = $derived(filterResultsByType(searchResults, "dictation"));
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

  {#if hasSearchQuery && searchResults.length > 0}
    <div class="flex items-center gap-2 text-xs text-text-muted">
      <span>{$t("meeting_history.search_results")}</span>
      <span class="pill pill-muted">{$t("meeting_history.results_count", { values: { count: searchResults.length } })}</span>
    </div>
  {/if}

  {#if filteredMeetings.length === 0 && dictationResults.length === 0}
    <EmptyState
      title={hasSearchQuery ? $t("meeting_history.no_matches_title") : $t("meeting_history.no_meetings_title")}
      message={hasSearchQuery ? $t("meeting_history.no_matches_msg") : $t("meeting_history.no_meetings_msg")}
    />
  {:else}
    <div class="flex flex-col gap-2">
      {#each filteredMeetings as meeting}
        {@const snippet = findSnippet(searchResults, "meeting", meeting.id)}
        <button
          onclick={() => onOpenMeeting(meeting.id)}
          class="w-full flex flex-col gap-1.5 px-4 py-3.5 rounded-default outline-1 outline-ghost-border bg-surface-2 text-left cursor-pointer transition-[background,outline-color] duration-150 hover:bg-surface-3 hover:outline-accent-blue/30"
        >
          <div class="flex items-center justify-between gap-4 w-full">
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
          </div>
          {#if snippet}
            <p class="search-highlight text-xs text-text-muted leading-relaxed mt-0.5 line-clamp-2">
              {@html snippet}
            </p>
          {/if}
        </button>
      {/each}

      {#if hasSearchQuery && dictationResults.length > 0}
        <div class="flex items-center gap-2 mt-2 text-xs text-text-muted">
          <Mic size={12} />
          <span>{$t("meeting_history.source_dictation")}</span>
        </div>
        {#each dictationResults as result}
          <div class="px-4 py-3 rounded-default outline-1 outline-ghost-border bg-surface-2">
            <div class="flex items-center gap-2 mb-1">
              <FileText size={14} class="text-text-muted shrink-0" />
              <span class="pill pill-muted text-xs">{$t("meeting_history.source_dictation")}</span>
            </div>
            <p class="search-highlight text-sm text-text-secondary leading-relaxed">
              {@html result.snippet}
            </p>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</div>
