import { describe, it, expect, vi } from 'vitest';
import { getAppState } from './app.svelte';
import { mockSettings } from '../test-helpers/fixtures';

const profile = {
  engine_id: "", engine_label: "", model_id: "", model_label: "", backend_id: "", backend_label: "",
};

describe('app store', () => {
  it('has correct initial state defaults', () => {
    const state = getAppState();
    expect(state.settingsOpen).toBe(false);
    expect(state.isRecording).toBe(false);
    expect(state.recordingMode).toBe('idle');
    expect(state.currentMeetingId).toBeNull();
    expect(state.selectedDevice).toBe('');
    expect(state.transcriptionRuntimePhase).toBe('download_required');
    expect(state.transcriptionModelOperationState).toBe('idle');
    expect(state.downloadFile).toBe('');
    expect(state.downloadCompletedFiles).toBe(0);
    expect(state.downloadTotalFiles).toBe(0);
    expect(state.settings.theme).toBe('dark');
    expect(state.settings.auto_paste).toBe(false);
    expect(state.settings.transcription_engine_id).toBe('');
  });

  it('openMeeting sets id and navigates to the meetings view', () => {
    const state = getAppState();
    state.openMeeting('test-meeting-id');
    expect(state.currentMeetingId).toBe('test-meeting-id');
  });

  it('settingsOpen toggles', () => {
    const state = getAppState();
    state.settingsOpen = true;
    expect(state.settingsOpen).toBe(true);
    state.settingsOpen = false;
    expect(state.settingsOpen).toBe(false);
  });

  it('settings setter updates correctly', () => {
    const state = getAppState();
    const newSettings = {
      ...mockSettings,
      theme: 'light' as const,
      locale: 'fr',
      auto_paste: true,
      paste_delay_ms: 200,
      ollama_model: 'llama3',
      debug_transcription: true,
      audio_device: 'mic-1',
      transcription_engine_id: 'whisper',
      transcription_model_id: 'whisper-base',
      transcription_backend_id: 'candle',
    };
    state.settings = newSettings;
    expect(state.settings.theme).toBe('light');
    expect(state.settings.auto_paste).toBe(true);
    expect(state.settings.debug_transcription).toBe(true);
    expect(state.settings.audio_device).toBe('mic-1');
    expect(state.settings.transcription_engine_id).toBe('whisper');
  });

  it('anchors the recording start clock while a session is live', () => {
    const state = getAppState();
    state.machineState = { state: "ready", data: { profile } };
    expect(state.recordingStartedAtMs).toBeNull();

    const before = Date.now();
    state.machineState = { state: "recording_meeting", data: { profile, session_id: 1, meeting_id: "m1" } };
    const anchor = state.recordingStartedAtMs;
    expect(anchor).not.toBeNull();
    expect(anchor as number).toBeGreaterThanOrEqual(before);

    // Stopping still shows the live card, so the anchor must survive it.
    state.machineState = { state: "stopping", data: { profile, was_recording: { meeting: { meeting_id: "m1" } } } };
    expect(state.recordingStartedAtMs).toBe(anchor);

    state.machineState = { state: "ready", data: { profile } };
    expect(state.recordingStartedAtMs).toBeNull();
  });

  it('anchors dictation the same way as a meeting', () => {
    const state = getAppState();
    state.machineState = { state: "ready", data: { profile } };
    expect(state.recordingStartedAtMs).toBeNull();

    state.machineState = { state: "recording_dictation", data: { profile, session_id: 1 } };
    const anchor = state.recordingStartedAtMs;
    expect(anchor).not.toBeNull();

    state.machineState = { state: "stopping", data: { profile, was_recording: "dictation" } };
    expect(state.recordingStartedAtMs).toBe(anchor);

    state.machineState = { state: "ready", data: { profile } };
    expect(state.recordingStartedAtMs).toBeNull();
  });

  it('re-anchors when a meeting is resumed as a new recording session', () => {
    const state = getAppState();
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date('2026-01-01T09:00:00Z'));
      state.machineState = { state: "recording_meeting", data: { profile, session_id: 1, meeting_id: "m1" } };
      expect(state.recordingStartedAtMs).toBe(Date.UTC(2026, 0, 1, 9, 0, 0));

      state.machineState = { state: "ready", data: { profile } };
      vi.setSystemTime(new Date('2026-01-01T09:20:00Z'));
      state.machineState = { state: "recording_meeting", data: { profile, session_id: 2, meeting_id: "m1" } };
      expect(state.recordingStartedAtMs).toBe(Date.UTC(2026, 0, 1, 9, 20, 0));
    } finally {
      vi.useRealTimers();
    }
    state.machineState = { state: "idle" };
  });

  it('recordingMode is derived from machineState', () => {
    const state = getAppState();
    state.machineState = { state: "recording_meeting", data: { profile: { engine_id: "", engine_label: "", model_id: "", model_label: "", backend_id: "", backend_label: "" }, session_id: 1, meeting_id: "m1" } };
    expect(state.recordingMode).toBe('meeting');
    state.machineState = { state: "recording_dictation", data: { profile: { engine_id: "", engine_label: "", model_id: "", model_label: "", backend_id: "", backend_label: "" }, session_id: 1 } };
    expect(state.recordingMode).toBe('dictation');
    state.machineState = { state: "idle" };
    expect(state.recordingMode).toBe('idle');
  });
});
