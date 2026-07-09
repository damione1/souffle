import { describe, it, expect, vi, beforeEach } from 'vitest';

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
  getLogTail,
  getDiagnosticsText,
  checkForUpdates,
  getAppVersion,
} from './diagnostics';

describe('diagnostics API', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('getLogTail passes max lines', async () => {
    mockInvoke.mockResolvedValue('line 1\nline 2');
    const result = await getLogTail(50);
    expect(mockInvoke).toHaveBeenCalledWith('get_log_tail', expect.objectContaining({ maxLines: 50 }), undefined);
    expect(result).toBe('line 1\nline 2');
  });

  it('getDiagnosticsText calls backend', async () => {
    mockInvoke.mockResolvedValue('diagnostics blob');
    const result = await getDiagnosticsText();
    expect(mockInvoke).toHaveBeenCalledWith('get_diagnostics_text', expect.any(Object), undefined);
    expect(result).toBe('diagnostics blob');
  });

  it('checkForUpdates returns result', async () => {
    const payload = {
      current_version: '0.1.0',
      latest_version: '0.2.0',
      update_available: true,
      release_notes: 'Notes',
      release_url: 'https://github.com/damione1/souffle/releases/tag/v0.2.0',
      check_error: null,
    };
    mockInvoke.mockResolvedValue(payload);
    const result = await checkForUpdates();
    expect(mockInvoke).toHaveBeenCalledWith('check_for_updates', expect.any(Object), undefined);
    expect(result).toEqual(payload);
  });

  it('getAppVersion returns version string', async () => {
    mockInvoke.mockResolvedValue('0.1.0');
    const result = await getAppVersion();
    expect(mockInvoke).toHaveBeenCalledWith('get_app_version', expect.any(Object), undefined);
    expect(result).toBe('0.1.0');
  });
});
