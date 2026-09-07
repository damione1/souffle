import { describe, expect, it } from "vitest";
import type { AppStateMachine, DictationEntry, MeetingListItem, TranscriptionProfile } from "../../types";
import { groupByDay, liveMeetingId, toTimelineItems } from "./controller.svelte";

const profile: TranscriptionProfile = {
  engine_id: "kyutai",
  engine_label: "Kyutai",
  model_id: "stt-1b-en_fr",
  model_label: "STT 1B",
  backend_id: "candle",
  backend_label: "Candle",
};

function dictation(id: string, timestamp: string): DictationEntry {
  return { id, text: `text ${id}`, timestamp };
}

function meeting(id: string, started_at: string): MeetingListItem {
  return {
    id,
    title: `Meeting ${id}`,
    started_at,
    duration_seconds: 60,
    has_summary: id === "m-summarized",
    summary_is_stale: false,
  };
}

describe("timeline items", () => {
  it("merges both kinds sorted newest first", () => {
    const items = toTimelineItems(
      [dictation("d1", "2026-06-12T10:00:00Z"), dictation("d2", "2026-06-10T08:00:00Z")],
      [meeting("m1", "2026-06-12T14:00:00Z"), meeting("m2", "2026-06-11T09:00:00Z")],
    );

    expect(items.map((item) => `${item.kind}:${item.id}`)).toEqual([
      "meeting:m1",
      "dictation:d1",
      "meeting:m2",
      "dictation:d2",
    ]);
  });

  it("carries summary state and duration for meetings", () => {
    const items = toTimelineItems([], [meeting("m-summarized", "2026-06-12T14:00:00Z")]);
    expect(items[0].hasSummary).toBe(true);
    expect(items[0].durationSeconds).toBe(60);
  });

  it("groups consecutive items by local day", () => {
    const items = toTimelineItems(
      [dictation("d1", "2026-06-12T23:30:00")],
      [meeting("m1", "2026-06-12T09:00:00"), meeting("m2", "2026-06-11T09:00:00")],
    );
    const groups = groupByDay(items);

    expect(groups).toHaveLength(2);
    expect(groups[0].day).toBe("2026-06-12");
    expect(groups[0].items.map((item) => item.id)).toEqual(["d1", "m1"]);
    expect(groups[1].day).toBe("2026-06-11");
  });
});

describe("liveMeetingId", () => {
  it("matches the recording meeting", () => {
    const state: AppStateMachine = {
      state: "recording_meeting",
      data: { profile, session_id: 1, meeting_id: "m1" },
    };
    expect(liveMeetingId(state)).toBe("m1");
  });

  it("matches the meeting still draining on stop", () => {
    const state: AppStateMachine = {
      state: "stopping",
      data: { profile, was_recording: { meeting: { meeting_id: "m1" } } },
    };
    expect(liveMeetingId(state)).toBe("m1");
  });

  it("ignores dictation stop and idle", () => {
    expect(liveMeetingId({
      state: "stopping",
      data: { profile, was_recording: "dictation" },
    })).toBeNull();
    expect(liveMeetingId({ state: "idle" })).toBeNull();
  });
});
