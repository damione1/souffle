import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import LiveSessionCard from "./LiveSessionCard.svelte";
import type { SystemAudioStatus } from "../../types";

vi.mock("../../api/generated", () => ({
  events: {
    audioLevel: { listen: vi.fn().mockResolvedValue(() => undefined) },
  },
}));

function stubMeeting(status: SystemAudioStatus | null) {
  return {
    liveTranscript: { committed: [], tail: [], tentative: [] },
    app: {
      recordingStartedAtMs: Date.now(),
      systemAudioStatus: status,
      settings: { auto_paste: false },
    },
    isStopping: false,
    idleSignal: null,
    stopRecording: vi.fn(),
    dismissIdle: vi.fn(),
    applyLiveParagraphEdit: vi.fn(),
    addDictionaryAlias: vi.fn(),
    notesDraft: "",
    notesSaveState: "idle" as const,
    onNotesChange: vi.fn(),
  };
}

function stubTranscription() {
  return {
    transcript: "",
    tentative: "",
    isStopping: false,
    toggleRecording: vi.fn(),
  };
}

describe("LiveSessionCard system-audio notice (SOU-119 AC5)", () => {
  it("spells out why a live meeting is mic only, not only the two-word badge", () => {
    render(LiveSessionCard, {
      props: {
        mode: "meeting",
        transcription: stubTranscription() as never,
        meeting: stubMeeting({
          active: false,
          reason: null,
          reason_code: "disabled",
          samples: 0,
          signal_samples: 0,
        }) as never,
      },
    });

    expect(screen.getAllByText("Mic only").length).toBeGreaterThan(0);
    expect(screen.getByText(/System audio capture is turned off in Settings/)).toBeTruthy();
  });

  it("does not warn when both lanes are up", () => {
    render(LiveSessionCard, {
      props: {
        mode: "meeting",
        transcription: stubTranscription() as never,
        meeting: stubMeeting({
          active: true,
          reason: null,
          reason_code: null,
          samples: 48_000,
          signal_samples: 48_000,
        }) as never,
      },
    });

    expect(screen.queryByText("Mic only")).toBeNull();
    expect(screen.queryByText(/System audio capture is turned off/)).toBeNull();
    expect(screen.getByText("System audio active")).toBeTruthy();
  });
});
