import { describe, it, expect } from 'vitest';
import { formatTimestamp, formatDuration, formatDate, formatBytes } from './format';

describe('formatTimestamp', () => {
  it('formats zero seconds', () => {
    expect(formatTimestamp(0)).toBe('0:00');
  });

  it('formats seconds with padding', () => {
    expect(formatTimestamp(5)).toBe('0:05');
  });

  it('formats minutes and seconds', () => {
    expect(formatTimestamp(65)).toBe('1:05');
  });

  it('formats large values', () => {
    expect(formatTimestamp(3661)).toBe('61:01');
  });

  it('truncates fractional seconds', () => {
    expect(formatTimestamp(1.9)).toBe('0:01');
  });
});

describe('formatDuration', () => {
  it('formats zero', () => {
    expect(formatDuration(0)).toBe('0:00');
  });

  it('pads seconds under a minute', () => {
    expect(formatDuration(5)).toBe('0:05');
  });

  it('formats minutes and seconds', () => {
    expect(formatDuration(45 * 60 + 7)).toBe('45:07');
  });

  it('rolls over to hours instead of counting past 59 minutes', () => {
    expect(formatDuration(3600)).toBe('1:00:00');
    expect(formatDuration(90 * 60 + 3)).toBe('1:30:03');
  });

  it('pads minutes once hours are shown', () => {
    expect(formatDuration(3600 + 5 * 60 + 9)).toBe('1:05:09');
  });

  it('handles multi-hour meetings', () => {
    expect(formatDuration(4 * 3600 + 2 * 60 + 1)).toBe('4:02:01');
  });

  it('truncates fractional seconds', () => {
    expect(formatDuration(61.9)).toBe('1:01');
  });

  it('never renders a negative span', () => {
    expect(formatDuration(-5)).toBe('0:00');
  });
});

describe('formatDate', () => {
  it('formats ISO date string', () => {
    const result = formatDate('2024-01-15T10:30:00Z');
    // Just verify it returns a non-empty string (locale-dependent)
    expect(result.length).toBeGreaterThan(0);
  });
});

describe('formatBytes', () => {
  it('formats sub-kilobyte sizes in bytes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
  });

  it('formats kilobytes without decimals above 10', () => {
    expect(formatBytes(482 * 1024)).toBe('482 KB');
  });

  it('formats sub-10 unit values with one decimal', () => {
    expect(formatBytes(1.5 * 1024 * 1024)).toBe('1.5 MB');
  });

  it('formats gigabytes', () => {
    expect(formatBytes(1.3 * 1024 * 1024 * 1024)).toBe('1.3 GB');
  });

  it('caps at terabytes for very large values', () => {
    expect(formatBytes(2048 * 1024 * 1024 * 1024 * 1024)).toBe('2048 TB');
  });
});
