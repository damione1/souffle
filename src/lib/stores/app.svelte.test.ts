import { describe, it, expect } from 'vitest';
import { getAppState } from './app.svelte';

describe('app store', () => {
  it('has correct initial state defaults', () => {
    const state = getAppState();
    expect(state.currentView).toBe('transcription');
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

  it('openMeeting sets id and navigates to meeting view', () => {
    const state = getAppState();
    state.openMeeting('test-meeting-id');
    expect(state.currentMeetingId).toBe('test-meeting-id');
    expect(state.currentView).toBe('meeting');
  });

  it('newMeeting clears id and sets meeting view', () => {
    const state = getAppState();
    // Set some existing state first
    state.openMeeting('existing-id');
    // Now create new meeting
    state.newMeeting();
    expect(state.currentMeetingId).toBeNull();
    expect(state.currentView).toBe('meeting');
  });

  it('settings setter updates correctly', () => {
    const state = getAppState();
    const newSettings = {
      theme: 'light' as const,
      auto_paste: true,
      paste_delay_ms: 200,
      ollama_url: 'http://localhost:11434',
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
