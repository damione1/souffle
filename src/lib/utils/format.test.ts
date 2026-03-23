import { describe, it, expect } from 'vitest';
import { formatTimestamp, formatDuration, formatDate } from './format';

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

  it('formats typical meeting duration', () => {
    expect(formatDuration(3600)).toBe('60:00');
  });
});

describe('formatDate', () => {
  it('formats ISO date string', () => {
    const result = formatDate('2024-01-15T10:30:00Z');
    // Just verify it returns a non-empty string (locale-dependent)
    expect(result.length).toBeGreaterThan(0);
  });
});
