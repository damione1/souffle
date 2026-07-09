import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock window.__TAURI_INTERNALS__ which is used by @tauri-apps/api/core invoke() and Channel
const mockInvoke = vi.fn();
let callbackId = 0;
Object.defineProperty(window, '__TAURI_INTERNALS__', {
  value: {
    invoke: mockInvoke,
    transformCallback: () => ++callbackId,
    metadata: { currentWebview: { windowLabel: 'main', label: 'main' }, currentWindow: { label: 'main' } },
  },
  writable: true,
});

import {
  getSettings,
  saveSettings,
  getShortcuts,
  saveShortcuts,
  listAudioDevices,
  selectAudioDevice,
} from './settings';

describe('settings API', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('getSettings returns settings object', async () => {
    const settings = { theme: 'dark', locale: '', auto_paste: false, paste_delay_ms: 100, ollama_url: '', ollama_model: '', debug_transcription: false, audio_device: null, transcription_engine_id: 'kyutai', transcription_model_id: 'stt-1b', transcription_backend_id: 'candle' };
    mockInvoke.mockResolvedValue(settings);

    const result = await getSettings();

    expect(mockInvoke).toHaveBeenCalledWith('get_settings', expect.any(Object), undefined);
    expect(result).toEqual(settings);
  });

  it('saveSettings passes settings object', async () => {
    mockInvoke.mockResolvedValue(null);
    const settings = { theme: 'light' as const, locale: '', auto_paste: true, paste_delay_ms: 200, ollama_url: 'http://localhost:11434', ollama_model: 'llama3', debug_transcription: false, audio_device: null, transcription_engine_id: 'kyutai', transcription_model_id: 'stt-1b', transcription_backend_id: 'candle', vad_enabled: true, filler_removal: true, stutter_collapse: false, dictionary_correction: true, capture_system_audio: true, calendar_integration_enabled: false, calendar_selected_ids: [], calendar_reminder_minutes: 2, model_unload_timeout_minutes: 0 };

    await saveSettings(settings);

    expect(mockInvoke).toHaveBeenCalledWith('save_settings', expect.objectContaining({ settings }), undefined);
  });

  it('getShortcuts returns shortcut settings', async () => {
    const shortcuts = { toggle: 'CmdOrCtrl+Shift+S', push_to_talk: 'CmdOrCtrl+Shift+Space' };
    mockInvoke.mockResolvedValue(shortcuts);

    const result = await getShortcuts();

    expect(mockInvoke).toHaveBeenCalledWith('get_shortcuts', expect.any(Object), undefined);
    expect(result).toEqual(shortcuts);
  });

  it('saveShortcuts passes shortcuts object', async () => {
    mockInvoke.mockResolvedValue(null);
    const shortcuts = { toggle: 'CmdOrCtrl+Shift+D', push_to_talk: 'CmdOrCtrl+Space' };

    await saveShortcuts(shortcuts);

    expect(mockInvoke).toHaveBeenCalledWith('save_shortcuts', expect.objectContaining({ shortcuts }), undefined);
  });

  it('listAudioDevices calls correct command', async () => {
    const devices = [{ name: 'MacBook Pro Microphone', is_default: true }];
    mockInvoke.mockResolvedValue(devices);

    const result = await listAudioDevices();

    expect(mockInvoke).toHaveBeenCalledWith('list_audio_devices', expect.any(Object), undefined);
    expect(result).toEqual(devices);
  });

  it('selectAudioDevice passes device name', async () => {
    mockInvoke.mockResolvedValue(null);

    await selectAudioDevice('External Mic');

    expect(mockInvoke).toHaveBeenCalledWith('select_audio_device', expect.objectContaining({ deviceName: 'External Mic' }), undefined);
  });
});
