import { listMeetings } from "../../api/meetings";
import { getAppState } from "../../stores/app.svelte";
import type { MeetingListItem } from "../../types";
import { createDebouncedSearch, errorMessage, matchedIdsForType } from "../../utils";

export function createMeetingHistoryController() {
  const app = getAppState();

  let meetings = $state<MeetingListItem[]>([]);
  let statusMessage = $state("");
  let searchQuery = $state("");
  const search = createDebouncedSearch(250, 20);

  const filteredMeetings = $derived.by(() => {
    const query = searchQuery.trim().toLowerCase();
    if (!query) return meetings;

    if (search.results.length > 0) {
      const matched = matchedIdsForType(search.results, "meeting");
      return meetings.filter((m) => matched.has(m.id));
    }

    // Fallback to title filtering while search is in progress
    return meetings.filter((m) => m.title.toLowerCase().includes(query));
  });

  async function mount() {
    await refreshMeetings();
  }

  async function refreshMeetings() {
    try {
      meetings = await listMeetings();
      statusMessage = "";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  function openMeeting(id: string) {
    app.openMeeting(id);
  }

  function onSearchQueryChange(query: string) {
    searchQuery = query;
    search.update(query);
  }

  return {
    get meetings() { return meetings; },
    get filteredMeetings() { return filteredMeetings; },
    get statusMessage() { return statusMessage; },
    get searchQuery() { return searchQuery; },
    set searchQuery(value: string) { onSearchQueryChange(value); },
    get searchResults() { return search.results; },
    get isSearching() { return search.isSearching; },
    mount,
    openMeeting,
  };
}
