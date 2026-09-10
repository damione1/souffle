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

  it("warns about a tap that ran all meeting and carried only silence", () => {
    // The frame count alone would call this healthy: the tap runs on the
    // device clock and delivers silence at full rate.
    const notice = pastSystemAudioNotice(verdict({ samples: 48_000 * 600, signal_samples: 0 }));
    expect(notice?.key).toBe("meeting_header.system_audio_reason_silent");
  });

  it("tells a silent tap from one that delivered nothing", () => {
    expect(pastSystemAudioNotice(verdict())?.key).toBe(
      "meeting_header.system_audio_reason_no_samples",
    );
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

  it("stands in for a silent leg too", () => {
    expect(provisionalSystemAudio(status({ samples: 48_000, signal_samples: 0 }))?.reason_code).toBe(
      "silent",
    );
  });

  it("has nothing to stand in for when the leg was healthy", () => {
    expect(provisionalSystemAudio(status({ samples: 48_000, signal_samples: 48_000 }))).toBeNull();
    expect(provisionalSystemAudio(null)).toBeNull();
  });
});
