import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DictationEntry, MeetingListItem } from "../../types";

const listDictationEntries = vi.fn<(limit?: number) => Promise<DictationEntry[]>>();
const listMeetings = vi.fn<() => Promise<MeetingListItem[]>>();

vi.mock("../../api/transcription", () => ({
  listDictationEntries: (...args: unknown[]) =>
    listDictationEntries(...(args as [])),
  deleteDictationEntry: vi.fn(),
}));

vi.mock("../../api/meetings", () => ({
  listMeetings: (...args: unknown[]) => listMeetings(...(args as [])),
  deleteMeeting: vi.fn(),
}));

const { createTimelineController, resetTimelineControllerForTest } = await import(
  "./controller.svelte"
);

function entry(id: string): DictationEntry {
  return { id, text: `text ${id}`, timestamp: `2026-09-08T00:00:0${id}Z` };
}

describe("timeline refresh generation", () => {
  beforeEach(() => {
    resetTimelineControllerForTest();
    listDictationEntries.mockReset();
    listMeetings.mockReset();
    listMeetings.mockResolvedValue([]);
  });

  it("does not let an earlier idle fetch hide a dictation saved after it started", async () => {
    let resolveStale: ((entries: DictationEntry[]) => void) | undefined;
    listDictationEntries.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveStale = resolve;
        }),
    );

    const timeline = createTimelineController();
    const stale = timeline.refresh();

    listDictationEntries.mockResolvedValueOnce([entry("2")]);
    await timeline.refresh();
    expect(timeline.groups.flatMap((group) => group.items.map((item) => item.id))).toEqual([
      "2",
    ]);

    resolveStale!([entry("1")]);
    await stale;

    expect(timeline.groups.flatMap((group) => group.items.map((item) => item.id))).toEqual([
      "2",
    ]);
  });
});
