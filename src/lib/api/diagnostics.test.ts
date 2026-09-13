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
  getReleaseNotesForVersion,
  getAppVersion,
} from './diagnostics';
import { COMMAND } from "../test-helpers/commands";

describe('diagnostics API', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('getLogTail passes max lines', async () => {
    mockInvoke.mockResolvedValue('line 1\nline 2');
    const result = await getLogTail(50);
    expect(mockInvoke).toHaveBeenCalledWith(COMMAND.getLogTail, expect.objectContaining({ maxLines: 50 }), undefined);
    expect(result).toBe('line 1\nline 2');
  });

  it('getDiagnosticsText calls backend', async () => {
    mockInvoke.mockResolvedValue('diagnostics blob');
    const result = await getDiagnosticsText();
    expect(mockInvoke).toHaveBeenCalledWith(COMMAND.getDiagnosticsText, expect.any(Object), undefined);
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
    expect(mockInvoke).toHaveBeenCalledWith(COMMAND.checkForUpdates, expect.any(Object), undefined);
    expect(result).toEqual(payload);
  });

  it('getReleaseNotesForVersion returns trimmed notes', async () => {
    mockInvoke.mockResolvedValue('  Release notes  ');
    const result = await getReleaseNotesForVersion('0.1.1');
    expect(mockInvoke).toHaveBeenCalledWith(
      COMMAND.getReleaseNotesForVersion,
      expect.objectContaining({ version: '0.1.1' }),
      undefined,
    );
    expect(result).toBe('Release notes');
  });

  it('getReleaseNotesForVersion returns null for empty notes', async () => {
    mockInvoke.mockResolvedValue('   ');
    const result = await getReleaseNotesForVersion('0.1.1');
    expect(result).toBeNull();
  });

  it('getAppVersion returns version string', async () => {
    mockInvoke.mockResolvedValue('0.1.0');
    const result = await getAppVersion();
    expect(mockInvoke).toHaveBeenCalledWith(COMMAND.getAppVersion, expect.any(Object), undefined);
    expect(result).toBe('0.1.0');
  });
});
