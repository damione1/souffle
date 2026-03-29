import { describe, it, expect, vi, beforeEach } from "vitest";
import type { MeetingListItem, SearchResult } from "../../types";

const mockListMeetings = vi.fn<() => Promise<MeetingListItem[]>>();
const mockSearchText = vi.fn<(query: string, limit?: number) => Promise<SearchResult[]>>();

vi.mock("../../api/meetings", () => ({
  listMeetings: (...args: unknown[]) => mockListMeetings(...(args as [])),
  searchText: (...args: unknown[]) => mockSearchText(...(args as [string, number?])),
}));

const mockOpenMeeting = vi.fn();

vi.mock("../../stores/app.svelte", () => ({
  getAppState: () => ({
    openMeeting: mockOpenMeeting,
  }),
}));

// Must import after vi.mock declarations (hoisted, but keeps intent clear)
const { createMeetingHistoryController } = await import(
  "./history-controller.svelte"
);

function makeMeetingItem(overrides: Partial<MeetingListItem> = {}): MeetingListItem {
  return {
    id: "m1",
    title: "Standup",
    started_at: "2025-06-01T10:00:00Z",
    duration_seconds: 300,
    has_summary: false,
    summary_is_stale: false,
    ...overrides,
  };
}

describe("MeetingHistoryController", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("mount loads meetings", async () => {
    const items = [makeMeetingItem(), makeMeetingItem({ id: "m2", title: "Retro" })];
    mockListMeetings.mockResolvedValue(items);

    const ctrl = createMeetingHistoryController();
    await ctrl.mount();

    expect(mockListMeetings).toHaveBeenCalledOnce();
    expect(ctrl.meetings).toEqual(items);
    expect(ctrl.statusMessage).toBe("");
  });

  it("mount error sets status message", async () => {
    mockListMeetings.mockRejectedValue("backend offline");

    const ctrl = createMeetingHistoryController();
    await ctrl.mount();

    expect(ctrl.statusMessage).toBe("backend offline");
    expect(ctrl.meetings).toEqual([]);
  });

  it("search query filters by title (fallback when no FTS results)", async () => {
    const items = [
      makeMeetingItem({ id: "m1", title: "Standup" }),
      makeMeetingItem({ id: "m2", title: "Retrospective" }),
      makeMeetingItem({ id: "m3", title: "Daily standup" }),
    ];
    mockListMeetings.mockResolvedValue(items);

    const ctrl = createMeetingHistoryController();
    await ctrl.mount();

    ctrl.searchQuery = "standup";
    // filteredMeetings is $derived — access triggers recomputation
    expect(ctrl.filteredMeetings).toHaveLength(2);
    expect(ctrl.filteredMeetings.map((m) => m.id)).toEqual(["m1", "m3"]);
  });

  it("openMeeting navigates via app store", () => {
    mockListMeetings.mockResolvedValue([]);

    const ctrl = createMeetingHistoryController();
    ctrl.openMeeting("m42");

    expect(mockOpenMeeting).toHaveBeenCalledWith("m42");
  });

  it("exposes searchResults and isSearching state", () => {
    mockListMeetings.mockResolvedValue([]);

    const ctrl = createMeetingHistoryController();
    expect(ctrl.searchResults).toEqual([]);
    expect(ctrl.isSearching).toBe(false);
  });
});
