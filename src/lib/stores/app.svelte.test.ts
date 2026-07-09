import { describe, it, expect } from 'vitest';
import { getAppState } from './app.svelte';

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
      theme: 'light' as const,
      locale: 'fr',
      auto_paste: true,
      paste_delay_ms: 200,
      ollama_url: 'http://localhost:11434',
      ollama_model: 'llama3',
      debug_transcription: true,
      audio_device: 'mic-1',
      clamshell_audio_device: null,
      transcription_engine_id: 'whisper',
      transcription_model_id: 'whisper-base',
      transcription_backend_id: 'candle',
      vad_enabled: true,
      filler_removal: true,
      stutter_collapse: false,
      dictionary_correction: true,
      capture_system_audio: true,
      calendar_integration_enabled: false,
      calendar_selected_ids: [],
      calendar_reminder_minutes: 2,
      model_unload_timeout_minutes: 0,
      meeting_autostop_enabled: true,
      meeting_autostop_minutes: 10,
      meeting_max_duration_minutes: 240,
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
