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

  it("does not call a meeting mic only before the leg has reported", () => {
    render(LiveSessionCard, {
      props: {
        mode: "meeting",
        transcription: stubTranscription() as never,
        meeting: stubMeeting(null) as never,
      },
    });

    expect(screen.queryByText("Mic only")).toBeNull();
    expect(screen.getByText("Checking system audio")).toBeTruthy();
  });
});

describe("LiveSessionCard system-audio dot (SOU-135 AC3)", () => {
  function dotClass(status: SystemAudioStatus | null): string {
    const { container } = render(LiveSessionCard, {
      props: {
        mode: "meeting",
        transcription: stubTranscription() as never,
        meeting: stubMeeting(status) as never,
      },
    });
    const dot = container.querySelector(".h-1\\.5.w-1\\.5.rounded-full");
    expect(dot).not.toBeNull();
    return dot!.className;
  }

  const active = {
    active: true,
    reason: null,
    reason_code: null,
    samples: 48_000,
    signal_samples: 48_000,
  } satisfies SystemAudioStatus;
  const unavailable = {
    active: false,
    reason: null,
    reason_code: "disabled",
    samples: 0,
    signal_samples: 0,
  } satisfies SystemAudioStatus;

  it("gives the three states three different dots", () => {
    const classes = [dotClass(active), dotClass(unavailable), dotClass(null)];
    expect(new Set(classes).size).toBe(3);
  });

  it("no longer paints pending like unavailable", () => {
    expect(dotClass(null)).not.toBe(dotClass(unavailable));
  });
});
