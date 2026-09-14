import { deleteMeeting, listMeetings } from "../../api/meetings";
import {
  deleteDictationEntry,
  listDictationEntries,
} from "../../api/transcription";
import { getAppState } from "../../stores/app.svelte";
import type {
  AppStateMachine,
  DictationEntry,
  MeetingListItem,
  SearchSource,
} from "../../types";
import {
  createDebouncedSearch,
  errorMessage,
  matchedIdsForType,
} from "../../utils";

export interface TimelineItem {
  /** Which store the row came from, typed on the contract's own search
   * source so a third one cannot appear here unannounced. */
  kind: SearchSource;
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

/** Meeting ID that is recording or still draining on stop, else null. */
export function liveMeetingId(state: AppStateMachine): string | null {
  if (state.state === "recording_meeting") return state.data.meeting_id;
  if (state.state === "stopping" && typeof state.data.was_recording === "object") {
    return state.data.was_recording.meeting.meeting_id;
  }
  return null;
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

/** UI filter: the contract's sources plus an "all" pseudo-value that only
 * exists in the timeline. Composed on `SearchSource` so a new source in Rust
 * shows up here instead of being silently unreachable. */
export type TimelineKindFilter = "all" | SearchSource;

function createTimelineControllerInstance() {
  const app = getAppState();

  let dictations = $state<DictationEntry[]>([]);
  let meetings = $state<MeetingListItem[]>([]);
  let statusMessage = $state("");
  let searchQuery = $state("");
  let kindFilter = $state<TimelineKindFilter>("all");
  let expandedDictationId = $state<string | null>(null);
  const search = createDebouncedSearch(250, 40);

  function currentItems(): TimelineItem[] {
    return toTimelineItems(dictations, meetings);
  }

  function currentFilteredItems(): TimelineItem[] {
    const query = searchQuery.trim().toLowerCase();
    const source = currentItems();
    const byKind = kindFilter === "all"
      ? source
      : source.filter((item) => item.kind === kindFilter);

    if (!query) return byKind;

    if (search.results.length > 0) {
      const matchedIds: Record<SearchSource, Set<string>> = {
        dictation: matchedIdsForType(search.results, "dictation"),
        meeting: matchedIdsForType(search.results, "meeting"),
      };
      return byKind.filter((item) => matchedIds[item.kind].has(item.id));
    }

    return byKind.filter((item) => item.title.toLowerCase().includes(query));
  }

  // HomeView refreshes as soon as the machine goes idle; dictation save
  // refreshes after the row is written. Without a generation token the
  // idle fetch (started before the insert) can land last and hide the
  // new entry until the next launch.
  let refreshGeneration = 0;

  async function refresh() {
    const generation = ++refreshGeneration;
    try {
      const [dictationEntries, meetingItems] = await Promise.all([
        listDictationEntries(200),
        listMeetings(),
      ]);
      if (generation !== refreshGeneration) return;
      dictations = dictationEntries;
      meetings = meetingItems;
      statusMessage = "";
    } catch (e) {
      if (generation !== refreshGeneration) return;
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

  function isLiveMeeting(id: string): boolean {
    return liveMeetingId(app.machineState) === id;
  }

  async function removeItem(item: TimelineItem) {
    try {
      if (item.kind === "meeting" && isLiveMeeting(item.id)) {
        throw new Error("Cannot delete a meeting while it is recording.");
      }
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
    // Getters, not `$derived`: this controller is a singleton. `$derived`
    // created while HomeView is mounted is owned by that view; Svelte 5.55+
    // freezes it when the view unmounts (settings used to destroy HomeView).
    get groups() { return groupByDay(currentFilteredItems()); },
    get isEmpty() { return currentItems().length === 0; },
    get hasMatches() { return currentFilteredItems().length > 0; },
    get searchQuery() { return searchQuery; },
    set searchQuery(value: string) { onSearchQueryChange(value); },
    get kindFilter() { return kindFilter; },
    set kindFilter(value: TimelineKindFilter) { kindFilter = value; },
    get searchResults() { return search.results; },
    get isSearching() { return search.isSearching; },
    get expandedDictationId() { return expandedDictationId; },
    isLiveMeeting,
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
