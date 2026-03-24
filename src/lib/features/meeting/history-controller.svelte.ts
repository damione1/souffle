import { listMeetings } from "../../api/meetings";
import { getAppState } from "../../stores/app.svelte";
import type { MeetingListItem } from "../../types";
import { errorMessage } from "../../utils";

export function createMeetingHistoryController() {
  const app = getAppState();

  let meetings = $state<MeetingListItem[]>([]);
  let statusMessage = $state("");
  let searchQuery = $state("");

  const filteredMeetings = $derived.by(() => {
    const query = searchQuery.trim().toLowerCase();
    if (!query) {
      return meetings;
    }

    return meetings.filter((meeting) => meeting.title.toLowerCase().includes(query));
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

  return {
    get meetings() { return meetings; },
    get filteredMeetings() { return filteredMeetings; },
    get statusMessage() { return statusMessage; },
    get searchQuery() { return searchQuery; },
    set searchQuery(value: string) { searchQuery = value; },
    mount,
    openMeeting,
  };
}
