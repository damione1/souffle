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
  listMeetings,
  getMeeting,
  startMeetingRecording,
  resumeMeetingRecording,
  stopMeetingRecording,
  summarizeMeeting,
  deleteMeeting,
  searchText,
  saveEditedTranscript,
  exportMeetingFilename,
  exportMeetingPreview,
  exportMeetingToFile,
} from './meetings';

describe('meetings API', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('listMeetings calls correct command', async () => {
    const meetings = [{ id: '1', title: 'Standup', started_at: '2024-01-01', duration_seconds: 600, has_summary: false, summary_is_stale: false }];
    mockInvoke.mockResolvedValue(meetings);

    const result = await listMeetings();

    expect(mockInvoke).toHaveBeenCalledWith('list_meetings', expect.any(Object), undefined);
    expect(result).toEqual(meetings);
  });

  it('getMeeting passes id', async () => {
    const meeting = { id: 'abc', title: 'Test', segments: [] };
    mockInvoke.mockResolvedValue(meeting);

    const result = await getMeeting('abc');

    expect(mockInvoke).toHaveBeenCalledWith('get_meeting', expect.objectContaining({ id: 'abc' }), undefined);
    expect(result).toEqual(meeting);
  });

  it('startMeetingRecording passes title, calendar context and channel', async () => {
    mockInvoke.mockResolvedValue(null);
    const onSegment = vi.fn();

    await startMeetingRecording('Daily Standup', null, onSegment);

    expect(mockInvoke).toHaveBeenCalledWith('start_meeting_recording', expect.objectContaining({ title: 'Daily Standup', calendar: null }), undefined);

    const calendar = { event_id: 'evt-1', participants: [{ name: 'Alice', email: null, is_organizer: true, is_current_user: false }] };
    await startMeetingRecording('Planning', calendar, onSegment);

    expect(mockInvoke).toHaveBeenCalledWith('start_meeting_recording', expect.objectContaining({ title: 'Planning', calendar }), undefined);
  });

  it('resumeMeetingRecording passes id and channel', async () => {
    mockInvoke.mockResolvedValue(null);
    const onSegment = vi.fn();

    await resumeMeetingRecording('meeting-1', onSegment);

    expect(mockInvoke).toHaveBeenCalledWith('resume_meeting_recording', expect.objectContaining({ meetingId: 'meeting-1' }), undefined);
  });

  it('stopMeetingRecording calls correct command', async () => {
    mockInvoke.mockResolvedValue('meeting-id-123');

    const result = await stopMeetingRecording();

    expect(mockInvoke).toHaveBeenCalledWith('stop_meeting_recording', expect.any(Object), undefined);
    expect(result).toBe('meeting-id-123');
  });

  it('summarizeMeeting passes id, model, template id, and channel', async () => {
    mockInvoke.mockResolvedValue(null);
    const onProgress = vi.fn();

    await summarizeMeeting('meeting-1', 'llama3', 'brief_overview', onProgress);

    expect(mockInvoke).toHaveBeenCalledWith(
      'summarize_meeting',
      expect.objectContaining({ id: 'meeting-1', model: 'llama3', templateId: 'brief_overview' }),
      undefined,
    );
  });

  it('deleteMeeting passes id', async () => {
    mockInvoke.mockResolvedValue(null);

    await deleteMeeting('meeting-1');

    expect(mockInvoke).toHaveBeenCalledWith('delete_meeting', expect.objectContaining({ id: 'meeting-1' }), undefined);
  });

  it('searchText passes query and limit', async () => {
    const results = [{ source_type: 'meeting', source_id: 'm1', snippet: '<mark>Hello</mark> world', rank: 1.0 }];
    mockInvoke.mockResolvedValue(results);

    const result = await searchText('Hello', 10);

    expect(mockInvoke).toHaveBeenCalledWith('search_text', expect.objectContaining({ query: 'Hello', limit: 10 }), undefined);
    expect(result).toEqual(results);
  });

  it('searchText defaults limit to null', async () => {
    mockInvoke.mockResolvedValue([]);

    await searchText('test');

    expect(mockInvoke).toHaveBeenCalledWith('search_text', expect.objectContaining({ query: 'test', limit: null }), undefined);
  });

  it('saveEditedTranscript passes id and text', async () => {
    mockInvoke.mockResolvedValue(null);

    await saveEditedTranscript('meeting-1', 'Edited text');

    expect(mockInvoke).toHaveBeenCalledWith('save_edited_transcript', expect.objectContaining({ id: 'meeting-1', editedTranscript: 'Edited text' }), undefined);
  });

  it('saveEditedTranscript passes null to clear', async () => {
    mockInvoke.mockResolvedValue(null);

    await saveEditedTranscript('meeting-1', null);

    expect(mockInvoke).toHaveBeenCalledWith('save_edited_transcript', expect.objectContaining({ id: 'meeting-1', editedTranscript: null }), undefined);
  });

  it('exportMeetingFilename passes id and format, returns the suggested name', async () => {
    mockInvoke.mockResolvedValue('2026-07-09-weekly-sync.md');

    const result = await exportMeetingFilename('meeting-1', 'markdown');

    expect(mockInvoke).toHaveBeenCalledWith('export_meeting_filename', expect.objectContaining({ id: 'meeting-1', format: 'markdown' }), undefined);
    expect(result).toBe('2026-07-09-weekly-sync.md');
  });

  it('exportMeetingPreview passes id and format, returns rendered text', async () => {
    mockInvoke.mockResolvedValue('# Weekly Sync\n');

    const result = await exportMeetingPreview('meeting-1', 'srt');

    expect(mockInvoke).toHaveBeenCalledWith('export_meeting_preview', expect.objectContaining({ id: 'meeting-1', format: 'srt' }), undefined);
    expect(result).toBe('# Weekly Sync\n');
  });

  it('exportMeetingToFile passes id, format and path', async () => {
    mockInvoke.mockResolvedValue(null);

    await exportMeetingToFile('meeting-1', 'vtt', '/tmp/export.vtt');

    expect(mockInvoke).toHaveBeenCalledWith('export_meeting_to_file', expect.objectContaining({ id: 'meeting-1', format: 'vtt', path: '/tmp/export.vtt' }), undefined);
  });
});
