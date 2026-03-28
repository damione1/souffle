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
  getTranscriptionCatalog,
  getModelStatus,
  downloadModel,
  loadModel,
  startStreamingTranscription,
  stopStreamingTranscription,
  listDictationEntries,
  addDictationEntry,
  deleteDictationEntry,
  clearDictationHistory,
  pasteText,
} from './transcription';

describe('transcription API', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('getTranscriptionCatalog calls correct command', async () => {
    const catalog = { engines: [], selected_engine_id: 'kyutai', selected_model_id: 'stt-1b' };
    mockInvoke.mockResolvedValue(catalog);

    const result = await getTranscriptionCatalog();

    expect(mockInvoke).toHaveBeenCalledWith('get_transcription_catalog', expect.any(Object), undefined);
    expect(result).toEqual(catalog);
  });

  it('getModelStatus calls correct command', async () => {
    const status = { profile: {}, downloaded: true, loaded: false, model_dir: '/tmp' };
    mockInvoke.mockResolvedValue(status);

    const result = await getModelStatus();

    expect(mockInvoke).toHaveBeenCalledWith('get_model_status', expect.any(Object), undefined);
    expect(result).toEqual(status);
  });

  it('downloadModel creates channel and invokes', async () => {
    mockInvoke.mockResolvedValue(null);
    const onProgress = vi.fn();

    await downloadModel(onProgress);

    expect(mockInvoke).toHaveBeenCalledWith('download_model', expect.objectContaining({ channel: expect.any(Object) }), undefined);
  });

  it('loadModel calls correct command', async () => {
    mockInvoke.mockResolvedValue(null);

    await loadModel();

    expect(mockInvoke).toHaveBeenCalledWith('load_model', expect.any(Object), undefined);
  });

  it('startStreamingTranscription creates channel and invokes', async () => {
    mockInvoke.mockResolvedValue(null);
    const onSegment = vi.fn();

    await startStreamingTranscription(onSegment);

    expect(mockInvoke).toHaveBeenCalledWith('start_transcription', expect.objectContaining({ channel: expect.any(Object) }), undefined);
  });

  it('stopStreamingTranscription calls correct command', async () => {
    mockInvoke.mockResolvedValue(null);

    await stopStreamingTranscription();

    expect(mockInvoke).toHaveBeenCalledWith('stop_transcription', expect.any(Object), undefined);
  });

  it('listDictationEntries passes limit', async () => {
    const entries = [{ id: '1', text: 'hello', timestamp: '2024-01-01' }];
    mockInvoke.mockResolvedValue(entries);

    const result = await listDictationEntries(10);

    expect(mockInvoke).toHaveBeenCalledWith('list_dictation_entries', expect.objectContaining({ limit: 10 }), undefined);
    expect(result).toEqual(entries);
  });

  it('addDictationEntry passes text', async () => {
    mockInvoke.mockResolvedValue(null);

    await addDictationEntry('test text');

    expect(mockInvoke).toHaveBeenCalledWith('add_dictation_entry', expect.objectContaining({ text: 'test text' }), undefined);
  });

  it('deleteDictationEntry passes id', async () => {
    mockInvoke.mockResolvedValue(null);

    await deleteDictationEntry('entry-1');

    expect(mockInvoke).toHaveBeenCalledWith('delete_dictation_entry', expect.objectContaining({ id: 'entry-1' }), undefined);
  });

  it('clearDictationHistory calls correct command', async () => {
    mockInvoke.mockResolvedValue(null);

    await clearDictationHistory();

    expect(mockInvoke).toHaveBeenCalledWith('clear_dictation_history', expect.any(Object), undefined);
  });

  it('pasteText passes text and delay', async () => {
    mockInvoke.mockResolvedValue(null);

    await pasteText('hello world', 150);

    expect(mockInvoke).toHaveBeenCalledWith('paste_text', expect.objectContaining({ text: 'hello world', delayMs: 150 }), undefined);
  });
});
