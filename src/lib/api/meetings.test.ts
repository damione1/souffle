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

  it('startMeetingRecording passes title and channel', async () => {
    mockInvoke.mockResolvedValue(null);
    const onSegment = vi.fn();

    await startMeetingRecording('Daily Standup', onSegment);

    expect(mockInvoke).toHaveBeenCalledWith('start_meeting_recording', expect.objectContaining({ title: 'Daily Standup' }), undefined);
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

  it('summarizeMeeting passes id, model, and channel', async () => {
    mockInvoke.mockResolvedValue(null);
    const onProgress = vi.fn();

    await summarizeMeeting('meeting-1', 'llama3', onProgress);

    expect(mockInvoke).toHaveBeenCalledWith('summarize_meeting', expect.objectContaining({ id: 'meeting-1', model: 'llama3' }), undefined);
  });

  it('deleteMeeting passes id', async () => {
    mockInvoke.mockResolvedValue(null);

    await deleteMeeting('meeting-1');

    expect(mockInvoke).toHaveBeenCalledWith('delete_meeting', expect.objectContaining({ id: 'meeting-1' }), undefined);
  });
});
