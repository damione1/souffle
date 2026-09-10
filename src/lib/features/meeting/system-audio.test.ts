import { describe, expect, it } from "vitest";
import type { MeetingSystemAudio, SystemAudioStatus } from "../../types";
import {
  liveSystemAudioNotice,
  pastSystemAudioNotice,
  provisionalSystemAudio,
} from "./system-audio";

function status(over: Partial<SystemAudioStatus> = {}): SystemAudioStatus {
  return { active: true, reason: null, reason_code: null, samples: 0, signal_samples: 0, ...over };
}

function verdict(over: Partial<MeetingSystemAudio> = {}): MeetingSystemAudio {
  return { active: true, reason: null, reason_code: null, samples: 0, signal_samples: 0, ...over };
}

describe("liveSystemAudioNotice", () => {
  it("says nothing while the system-audio leg is up", () => {
    expect(liveSystemAudioNotice(status())).toBeNull();
    expect(liveSystemAudioNotice(status({ samples: 48_000, signal_samples: 48_000 }))).toBeNull();
  });

  it("names the reason a meeting is running mic only", () => {
    const notice = liveSystemAudioNotice(status({ active: false, reason_code: "disabled" }));
    expect(notice?.key).toBe("meeting_header.system_audio_reason_disabled");
    expect(notice?.detail).toBeNull();
  });

  it("says the permission was refused rather than a generic failure", () => {
    const notice = liveSystemAudioNotice(
      status({
        active: false,
        reason_code: "permission_denied",
        reason: "AudioHardwareCreateProcessTap failed (560227702)",
      }),
    );
    expect(notice?.key).toBe("meeting_header.system_audio_reason_permission_denied");
    expect(notice?.detail).toBe("AudioHardwareCreateProcessTap failed (560227702)");
  });

  it("still says something when the backend sent no code", () => {
    expect(liveSystemAudioNotice(status({ active: false }))?.key).toBe(
      "meeting_header.system_audio_reason_unknown",
    );
  });

  it("has nothing to say before the backend answers", () => {
    expect(liveSystemAudioNotice(null)).toBeNull();
  });
});

describe("pastSystemAudioNotice", () => {
  it("says nothing about a meeting whose system-audio leg carried sound", () => {
    expect(
      pastSystemAudioNotice(verdict({ samples: 96_000, signal_samples: 96_000 })),
    ).toBeNull();
  });

  it("says nothing about a meeting recorded before the leg was tracked", () => {
    expect(pastSystemAudioNotice(null)).toBeNull();
  });

  it("explains a meeting recorded mic only", () => {
    const notice = pastSystemAudioNotice(
      verdict({ active: false, reason_code: "tap_lost", reason: "device vanished" }),
    );
    expect(notice?.key).toBe("meeting_header.system_audio_reason_tap_lost");
    expect(notice?.detail).toBe("device vanished");
  });

  it("does not warn about a tap that ran, even when it carried only silence", () => {
    // AC6: both lanes were captured. AC8 persists Silent/NoSamples on the
    // meeting for later analysis; it is not a "mic only" banner.
    expect(
      pastSystemAudioNotice(verdict({ samples: 48_000 * 600, signal_samples: 0 })),
    ).toBeNull();
    expect(pastSystemAudioNotice(verdict())).toBeNull();
  });
});

describe("provisionalSystemAudio", () => {
  it("stands in for the header row a stop loads before the real save", () => {
    const audio = provisionalSystemAudio(
      status({ active: false, reason_code: "probe_failed", reason: "boom" }),
    );
    expect(audio).toEqual({
      active: false,
      reason: "boom",
      reason_code: "probe_failed",
      samples: 0,
      signal_samples: 0,
    });
    expect(pastSystemAudioNotice(audio)?.key).toBe(
      "meeting_header.system_audio_reason_probe_failed",
    );
  });

  it("does not stand in for a tap that ran, silent or not", () => {
    expect(provisionalSystemAudio(status({ samples: 48_000, signal_samples: 0 }))).toBeNull();
    expect(provisionalSystemAudio(status({ samples: 48_000, signal_samples: 48_000 }))).toBeNull();
    expect(provisionalSystemAudio(null)).toBeNull();
  });
});
