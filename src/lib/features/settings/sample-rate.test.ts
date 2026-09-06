import { describe, expect, it } from "vitest";
import type { AudioInputDevice } from "../../types";
import en from "../../i18n/en.json";
import {
  formatSampleRateHz,
  resolveSampleRateDeviceUid,
  sampleRateBlocksConferencing,
} from "./sample-rate";

const builtin: AudioInputDevice = {
  uid: "builtin",
  name: "Built-in",
  transport: "built_in",
  is_default: true,
};
const usb: AudioInputDevice = {
  uid: "usb",
  name: "USB Mic",
  transport: "usb",
  is_default: false,
};

describe("sample-rate warning copy", () => {
  it("points at Teams/Zoom and Audio MIDI Setup", () => {
    expect(en.settings_audio.sample_rate_high_warning).toMatch(/Teams and Zoom/);
    expect(en.settings_audio.sample_rate_high_warning).toMatch(/Audio MIDI Setup/);
  });
});

describe("sampleRateBlocksConferencing", () => {
  it("warns only above 48 kHz", () => {
    expect(sampleRateBlocksConferencing(96_000)).toBe(true);
    expect(sampleRateBlocksConferencing(48_001)).toBe(true);
    expect(sampleRateBlocksConferencing(48_000)).toBe(false);
    expect(sampleRateBlocksConferencing(44_100)).toBe(false);
    expect(sampleRateBlocksConferencing(null)).toBe(false);
    expect(sampleRateBlocksConferencing(undefined)).toBe(false);
  });
});

describe("formatSampleRateHz", () => {
  it("formats common rates", () => {
    expect(formatSampleRateHz(96_000)).toBe("96 kHz");
    expect(formatSampleRateHz(48_000)).toBe("48 kHz");
    expect(formatSampleRateHz(44_100)).toBe("44.1 kHz");
    expect(formatSampleRateHz(16_000)).toBe("16 kHz");
  });
});

describe("resolveSampleRateDeviceUid", () => {
  it("uses the connected pin when one is selected", () => {
    expect(resolveSampleRateDeviceUid("usb", [builtin, usb])).toBe("usb");
  });

  it("returns null when the pin is not connected", () => {
    expect(resolveSampleRateDeviceUid("ghost", [builtin, usb])).toBeNull();
  });

  it("uses the default input when selection is automatic", () => {
    expect(resolveSampleRateDeviceUid("", [usb, builtin])).toBe("builtin");
  });
});
