/**
 * Mock factories for `src/lib/api/*` wrapper modules.
 *
 * Usage:
 *   vi.mock('$lib/api/transcription', () => createTranscriptionApiMock());
 *   vi.mock('$lib/api/meetings',      () => createMeetingsApiMock());
 *   vi.mock('$lib/api/settings',      () => createSettingsApiMock());
 *   vi.mock('$lib/api/summary',      () => createSummaryApiMock());
 *
 * Pass `overrides` to replace individual default return values.
 */

import { vi } from "vitest";
import type {
  AppSettings,
  AudioInputDevice,
  DictationEntry,
  MeetingListItem,
  MeetingTranscript,
  SummaryProvidersStatus,
  ShortcutSettings,
  TranscriptionCatalog,
  TranscriptionRuntimeStatus,
  TranscriptionSegment,
} from "../types";
import {
  mockCatalog,
  mockDictationEntry,
  mockMeeting,
  mockMeetingList,
  mockSummaryProvidersStatus,
  mockRuntimeStatus,
  mockSegment,
  mockSettings,
  mockShortcuts,
} from "./fixtures";

// ---------------------------------------------------------------------------
// src/lib/api/transcription.ts
// ---------------------------------------------------------------------------

export interface TranscriptionApiMock {
  getTranscriptionCatalog: ReturnType<typeof vi.fn<() => Promise<TranscriptionCatalog>>>;
  getModelStatus: ReturnType<typeof vi.fn<() => Promise<TranscriptionRuntimeStatus>>>;
  downloadModel: ReturnType<typeof vi.fn<(onProgress: (p: unknown) => void) => Promise<void>>>;
  loadModel: ReturnType<typeof vi.fn<() => Promise<void>>>;
  startStreamingTranscription: ReturnType<typeof vi.fn<(onSegment: (s: TranscriptionSegment) => void) => Promise<void>>>;
  stopStreamingTranscription: ReturnType<typeof vi.fn<() => Promise<void>>>;
  listDictationEntries: ReturnType<typeof vi.fn<(limit?: number) => Promise<DictationEntry[]>>>;
  addDictationEntry: ReturnType<typeof vi.fn<(text: string) => Promise<void>>>;
  deleteDictationEntry: ReturnType<typeof vi.fn<(id: string) => Promise<void>>>;
  clearDictationHistory: ReturnType<typeof vi.fn<() => Promise<void>>>;
  pasteText: ReturnType<typeof vi.fn<(text: string, delayMs: number, method?: string) => Promise<void>>>;
}

export function createTranscriptionApiMock(
  overrides?: Partial<TranscriptionApiMock>,
): TranscriptionApiMock {
  return {
    getTranscriptionCatalog: vi.fn<() => Promise<TranscriptionCatalog>>().mockResolvedValue(mockCatalog),
    getModelStatus: vi.fn<() => Promise<TranscriptionRuntimeStatus>>().mockResolvedValue(mockRuntimeStatus),
    downloadModel: vi.fn<(onProgress: (p: unknown) => void) => Promise<void>>().mockResolvedValue(undefined),
    loadModel: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    startStreamingTranscription: vi.fn<(onSegment: (s: TranscriptionSegment) => void) => Promise<void>>().mockResolvedValue(undefined),
    stopStreamingTranscription: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    listDictationEntries: vi.fn<(limit?: number) => Promise<DictationEntry[]>>().mockResolvedValue([mockDictationEntry]),
    addDictationEntry: vi.fn<(text: string) => Promise<void>>().mockResolvedValue(undefined),
    deleteDictationEntry: vi.fn<(id: string) => Promise<void>>().mockResolvedValue(undefined),
    clearDictationHistory: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
    pasteText: vi.fn<(text: string, delayMs: number, method?: string) => Promise<void>>().mockResolvedValue(undefined),
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// src/lib/api/meetings.ts
// ---------------------------------------------------------------------------

export interface MeetingsApiMock {
  listMeetings: ReturnType<typeof vi.fn<() => Promise<MeetingListItem[]>>>;
  getMeeting: ReturnType<typeof vi.fn<(id: string) => Promise<MeetingTranscript>>>;
  startMeetingRecording: ReturnType<typeof vi.fn<(title: string, calendar: unknown, onSegment: (s: TranscriptionSegment) => void) => Promise<void>>>;
  resumeMeetingRecording: ReturnType<typeof vi.fn<(id: string, onSegment: (s: TranscriptionSegment) => void) => Promise<void>>>;
  stopMeetingRecording: ReturnType<typeof vi.fn<() => Promise<string>>>;
  summarizeMeeting: ReturnType<typeof vi.fn<(id: string, model: string, templateId: string | null, onProgress: (p: unknown) => void) => Promise<void>>>;
  deleteMeeting: ReturnType<typeof vi.fn<(id: string) => Promise<void>>>;
}

export function createMeetingsApiMock(
  overrides?: Partial<MeetingsApiMock>,
): MeetingsApiMock {
  return {
    listMeetings: vi.fn<() => Promise<MeetingListItem[]>>().mockResolvedValue(mockMeetingList),
    getMeeting: vi.fn<(id: string) => Promise<MeetingTranscript>>().mockResolvedValue(mockMeeting),
    startMeetingRecording: vi.fn<(title: string, calendar: unknown, onSegment: (s: TranscriptionSegment) => void) => Promise<void>>().mockResolvedValue(undefined),
    resumeMeetingRecording: vi.fn<(id: string, onSegment: (s: TranscriptionSegment) => void) => Promise<void>>().mockResolvedValue(undefined),
    stopMeetingRecording: vi.fn<() => Promise<string>>().mockResolvedValue("meeting-001"),
    summarizeMeeting: vi.fn<(id: string, model: string, templateId: string | null, onProgress: (p: unknown) => void) => Promise<void>>().mockResolvedValue(undefined),
    deleteMeeting: vi.fn<(id: string) => Promise<void>>().mockResolvedValue(undefined),
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// src/lib/api/settings.ts
// ---------------------------------------------------------------------------

export interface SettingsApiMock {
  getSettings: ReturnType<typeof vi.fn<() => Promise<AppSettings>>>;
  saveSettings: ReturnType<typeof vi.fn<(settings: AppSettings) => Promise<void>>>;
  getShortcuts: ReturnType<typeof vi.fn<() => Promise<ShortcutSettings>>>;
  saveShortcuts: ReturnType<typeof vi.fn<(shortcuts: ShortcutSettings) => Promise<void>>>;
  listAudioDevices: ReturnType<typeof vi.fn<() => Promise<AudioInputDevice[]>>>;
  selectAudioDevice: ReturnType<typeof vi.fn<(deviceUid: string) => Promise<void>>>;
}

export function createSettingsApiMock(
  overrides?: Partial<SettingsApiMock>,
): SettingsApiMock {
  return {
    getSettings: vi.fn<() => Promise<AppSettings>>().mockResolvedValue(mockSettings),
    saveSettings: vi.fn<(settings: AppSettings) => Promise<void>>().mockResolvedValue(undefined),
    getShortcuts: vi.fn<() => Promise<ShortcutSettings>>().mockResolvedValue(mockShortcuts),
    saveShortcuts: vi.fn<(shortcuts: ShortcutSettings) => Promise<void>>().mockResolvedValue(undefined),
    listAudioDevices: vi.fn<() => Promise<AudioInputDevice[]>>().mockResolvedValue([
      { uid: "builtin-mic", name: "Built-in Microphone", transport: "built_in", is_default: true },
      { uid: "usb-mic", name: "External USB Mic", transport: "usb", is_default: false },
    ]),
    selectAudioDevice: vi.fn<(deviceUid: string) => Promise<void>>().mockResolvedValue(undefined),
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// src/lib/api/summary.ts
// ---------------------------------------------------------------------------

export interface SummaryApiMock {
  getSummaryProvidersStatus: ReturnType<typeof vi.fn<() => Promise<SummaryProvidersStatus>>>;
}

export function createSummaryApiMock(
  overrides?: Partial<SummaryApiMock>,
): SummaryApiMock {
  return {
    getSummaryProvidersStatus: vi.fn<() => Promise<SummaryProvidersStatus>>().mockResolvedValue(mockSummaryProvidersStatus),
    ...overrides,
  };
}
