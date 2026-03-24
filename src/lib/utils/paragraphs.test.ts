import { describe, it, expect } from 'vitest';
import { groupIntoParagraphs } from './paragraphs';
import type { TranscriptionSegment } from '../types';

function seg(text: string, start: number, end?: number): TranscriptionSegment {
  return {
    text,
    start_time: start,
    end_time: end ?? start + 0.5,
    is_final: true,
    language: null,
    confidence: null,
  };
}

describe('groupIntoParagraphs', () => {
  it('returns empty array for empty input', () => {
    expect(groupIntoParagraphs([], 1.5)).toEqual([]);
  });

  it('groups single segment', () => {
    const result = groupIntoParagraphs([seg('Hello', 0)], 1.5);
    expect(result).toHaveLength(1);
    expect(result[0].text).toBe('Hello');
    expect(result[0].timestamp).toBe('0:00');
  });

  it('joins segments without pause', () => {
    const segments = [seg('Hello', 0), seg('world', 0.5)];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(1);
    expect(result[0].text).toBe('Hello world');
  });

  it('creates new paragraph on pause after sentence end', () => {
    const segments = [
      seg('Hello world.', 0),
      seg('New paragraph', 2.0), // 2s gap > 1.5s threshold, previous ends with .
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(2);
    expect(result[0].text).toBe('Hello world.');
    expect(result[1].text).toBe('New paragraph');
  });

  it('does not split on pause without sentence ending', () => {
    const segments = [
      seg('Hello world', 0), // no period
      seg('more text', 2.0), // 2s gap but no sentence ending
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(1);
  });

  it('handles question mark as sentence boundary', () => {
    const segments = [
      seg('Is it working?', 0),
      seg('Yes', 2.0),
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(2);
  });
});
