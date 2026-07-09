import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock window.__TAURI_INTERNALS__ which is used by @tauri-apps/api/core invoke()
const mockInvoke = vi.fn();
Object.defineProperty(window, '__TAURI_INTERNALS__', {
  value: {
    invoke: mockInvoke,
    transformCallback: () => 0,
    metadata: { currentWebview: { windowLabel: 'main', label: 'main' }, currentWindow: { label: 'main' } },
  },
  writable: true,
});

import { getDataStats, exportArchive, revealDataDir, getMcpSetupInfo, testMcpConnection } from './data';

describe('data API', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('getDataStats returns the stats object', async () => {
    const stats = { db_size_bytes: 12345, meeting_count: 3, dictation_count: 7 };
    mockInvoke.mockResolvedValue(stats);

    const result = await getDataStats();

    expect(mockInvoke).toHaveBeenCalledWith('get_data_stats', expect.any(Object), undefined);
    expect(result).toEqual(stats);
  });

  it('exportArchive passes the destination directory', async () => {
    mockInvoke.mockResolvedValue(null);

    await exportArchive('/Users/damien/Desktop');

    expect(mockInvoke).toHaveBeenCalledWith(
      'export_archive',
      expect.objectContaining({ destDir: '/Users/damien/Desktop' }),
      undefined,
    );
  });

  it('exportArchive throws on a validation error', async () => {
    mockInvoke.mockRejectedValue('Destination is not a directory: /nope');

    await expect(exportArchive('/nope')).rejects.toBe('Destination is not a directory: /nope');
  });

  it('revealDataDir invokes the command with no arguments', async () => {
    mockInvoke.mockResolvedValue(null);

    await revealDataDir();

    expect(mockInvoke).toHaveBeenCalledWith('reveal_data_dir', expect.any(Object), undefined);
  });

  it('getMcpSetupInfo returns the setup object', async () => {
    const info = {
      binary_path: '/tmp/souffle-mcp',
      exists: true,
      claude_desktop_snippet: '{"mcpServers":{"souffle":{"command":"/tmp/souffle-mcp"}}}',
      claude_code_command: 'claude mcp add souffle /tmp/souffle-mcp',
    };
    mockInvoke.mockResolvedValue(info);

    const result = await getMcpSetupInfo();

    expect(mockInvoke).toHaveBeenCalledWith('get_mcp_setup_info', expect.any(Object), undefined);
    expect(result).toEqual(info);
  });

  it('testMcpConnection returns discovered tool names', async () => {
    mockInvoke.mockResolvedValue('list_meetings, get_meeting');

    const result = await testMcpConnection();

    expect(mockInvoke).toHaveBeenCalledWith('test_mcp_connection', expect.any(Object), undefined);
    expect(result).toBe('list_meetings, get_meeting');
  });
});
