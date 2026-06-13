import { deleteMeeting, listMeetings } from "../../api/meetings";
import {
  deleteDictationEntry,
  listDictationEntries,
} from "../../api/transcription";
import { getAppState } from "../../stores/app.svelte";
import type { DictationEntry, MeetingListItem } from "../../types";
import {
  createDebouncedSearch,
  errorMessage,
  matchedIdsForType,
} from "../../utils";

export interface TimelineItem {
  kind: "dictation" | "meeting";
  id: string;
  /** Meeting title, or the dictation text (also used as the excerpt). */
  title: string;
  /** RFC3339 timestamp the item is sorted and grouped by. */
  at: string;
  durationSeconds: number | null;
  hasSummary: boolean;
  summaryIsStale: boolean;
}

export interface TimelineGroup {
  /** YYYY-MM-DD key in local time. */
  day: string;
  items: TimelineItem[];
}

function dayKey(iso: string): string {
  const date = new Date(iso);
  const month = `${date.getMonth() + 1}`.padStart(2, "0");
  const dayOfMonth = `${date.getDate()}`.padStart(2, "0");
  return `${date.getFullYear()}-${month}-${dayOfMonth}`;
}

export function toTimelineItems(
  dictations: DictationEntry[],
  meetings: MeetingListItem[],
): TimelineItem[] {
  const items: TimelineItem[] = [
    ...dictations.map((entry): TimelineItem => ({
      kind: "dictation",
      id: entry.id,
      title: entry.text,
      at: entry.timestamp,
      durationSeconds: null,
      hasSummary: false,
      summaryIsStale: false,
    })),
    ...meetings.map((meeting): TimelineItem => ({
      kind: "meeting",
      id: meeting.id,
      title: meeting.title,
      at: meeting.started_at,
      durationSeconds: meeting.duration_seconds,
      hasSummary: meeting.has_summary,
      summaryIsStale: meeting.summary_is_stale,
    })),
  ];
  return items.sort((a, b) => b.at.localeCompare(a.at));
}

export function groupByDay(items: TimelineItem[]): TimelineGroup[] {
  const groups: TimelineGroup[] = [];
  for (const item of items) {
    const day = dayKey(item.at);
    const last = groups[groups.length - 1];
    if (last && last.day === day) {
      last.items.push(item);
    } else {
      groups.push({ day, items: [item] });
    }
  }
  return groups;
}

function createTimelineControllerInstance() {
  const app = getAppState();

  let dictations = $state<DictationEntry[]>([]);
  let meetings = $state<MeetingListItem[]>([]);
  let statusMessage = $state("");
  let searchQuery = $state("");
  let expandedDictationId = $state<string | null>(null);
  const search = createDebouncedSearch(250, 40);

  const items = $derived(toTimelineItems(dictations, meetings));

  const filteredItems = $derived.by(() => {
    const query = searchQuery.trim().toLowerCase();
    if (!query) return items;

    if (search.results.length > 0) {
      const matchedDictations = matchedIdsForType(search.results, "dictation");
      const matchedMeetings = matchedIdsForType(search.results, "meeting");
      return items.filter((item) =>
        item.kind === "dictation"
          ? matchedDictations.has(item.id)
          : matchedMeetings.has(item.id),
      );
    }

    // Local fallback while the FTS query is in flight.
    return items.filter((item) => item.title.toLowerCase().includes(query));
  });

  const groups = $derived(groupByDay(filteredItems));

  async function refresh() {
    try {
      const [dictationEntries, meetingItems] = await Promise.all([
        listDictationEntries(200),
        listMeetings(),
      ]);
      dictations = dictationEntries;
      meetings = meetingItems;
      statusMessage = "";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  function onSearchQueryChange(query: string) {
    searchQuery = query;
    search.update(query);
  }

  function toggleDictation(id: string) {
    expandedDictationId = expandedDictationId === id ? null : id;
  }

  async function removeItem(item: TimelineItem) {
    try {
      if (item.kind === "dictation") {
        await deleteDictationEntry(item.id);
        if (expandedDictationId === item.id) expandedDictationId = null;
      } else {
        await deleteMeeting(item.id);
      }
      await refresh();
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  function openItem(item: TimelineItem) {
    if (item.kind === "meeting") {
      app.openMeeting(item.id);
    } else {
      toggleDictation(item.id);
    }
  }

  return {
    get app() { return app; },
    get statusMessage() { return statusMessage; },
    get groups() { return groups; },
    get isEmpty() { return items.length === 0; },
    get hasMatches() { return filteredItems.length > 0; },
    get searchQuery() { return searchQuery; },
    set searchQuery(value: string) { onSearchQueryChange(value); },
    get searchResults() { return search.results; },
    get isSearching() { return search.isSearching; },
    get expandedDictationId() { return expandedDictationId; },
    refresh,
    openItem,
    removeItem,
  };
}

// Singleton so the timeline survives detail-view round-trips without reloads.
let instance: ReturnType<typeof createTimelineControllerInstance> | null = null;

export function createTimelineController() {
  if (!instance) {
    instance = createTimelineControllerInstance();
  }
  return instance;
}

/** Reset the singleton for testing. */
export function resetTimelineControllerForTest() {
  instance = null;
}
