import { describe, it, expect } from 'vitest';
import { buildMeetingTranscriptBlocks, groupIntoParagraphs } from './paragraphs';
import type { MeetingRecordingSession, TranscriptionSegment } from '../types';

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

function session(id: string, start: number, end: number): MeetingRecordingSession {
  return {
    id,
    started_at: new Date(`2026-03-27T10:0${start}:00Z`).toISOString(),
    ended_at: new Date(`2026-03-27T10:0${end}:00Z`).toISOString(),
    duration_seconds: (end - start) * 60,
    start_segment_index: start,
    end_segment_index: end,
  };
}

function dseg(
  text: string,
  start: number,
  speaker: 'me' | 'them',
): TranscriptionSegment {
  return { ...seg(text, start), speaker };
}

describe('groupIntoParagraphs diarization', () => {
  it('breaks on speaker change and tags each paragraph', () => {
    const result = groupIntoParagraphs(
      [dseg('hi there', 0, 'me'), dseg('hello back', 2, 'them'), dseg('great', 4, 'me')],
      1.5,
    );
    expect(result.map((p) => p.speaker)).toEqual(['me', 'them', 'me']);
    expect(result[0].text).toBe('hi there');
    expect(result[1].text).toBe('hello back');
  });

  it('orders interleaved speakers chronologically', () => {
    // Them arrives in the array before Me but starts later.
    const result = groupIntoParagraphs(
      [dseg('second', 3, 'them'), dseg('first', 1, 'me')],
      1.5,
    );
    expect(result[0].text).toBe('first');
    expect(result[0].speaker).toBe('me');
    expect(result[1].text).toBe('second');
    expect(result[1].speaker).toBe('them');
  });

  it('leaves non-diarized segments untagged and unsorted', () => {
    const result = groupIntoParagraphs([seg('a', 0), seg('b', 1)], 1.5);
    expect(result[0].speaker == null).toBe(true);
  });
});

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

  it('breaks continuous speech after max sentences without any pause', () => {
    // 6 sentences, 0.5s apart — no pause ever reaches the 1.5s threshold.
    const segments = Array.from({ length: 6 }, (_, i) =>
      seg(`Sentence number ${i + 1}.`, i * 0.5),
    );
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(2);
    expect(result[0].text).toContain('Sentence number 4.');
    expect(result[1].text).toContain('Sentence number 5.');
  });

  it('breaks at the next sentence end once the soft length cap is reached', () => {
    const longSentence = `${'word '.repeat(100).trim()}.`; // ~500 chars
    const segments = [
      seg(longSentence, 0),
      seg('Short follow-up.', 0.5),
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(2);
    expect(result[1].text).toBe('Short follow-up.');
  });

  it('hard-breaks punctuation-less streams at the length ceiling', () => {
    // 300 words, no punctuation at all — must still split eventually.
    const segments = Array.from({ length: 300 }, (_, i) =>
      seg(`word${i}`, i * 0.1),
    );
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result.length).toBeGreaterThan(1);
    for (const paragraph of result) {
      expect(paragraph.text.length).toBeLessThan(800);
    }
  });

  it('counts multiple sentences inside one segment (batch engines)', () => {
    // Whisper/Parakeet style: each segment holds several sentences.
    const segments = [
      seg('First one. Second one. Third one.', 0),
      seg('Fourth one. Fifth one.', 5),
      seg('Sixth one.', 10),
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result.length).toBeGreaterThan(1);
  });

  it('ignores negative gaps from legacy window-relative timestamps', () => {
    const segments = [
      seg('From window one.', 4.0, 4.5),
      seg('From window two', 0.2), // timestamp restarted — not a pause
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(1);
  });
});

describe('buildMeetingTranscriptBlocks', () => {
  it('inserts a session break between saved recording sessions', () => {
    const segments = [
      seg('First session.', 0),
      seg('Second session starts.', 0),
    ];

    const result = buildMeetingTranscriptBlocks(
      segments,
      [
        session('session-1', 0, 1),
        session('session-2', 1, 2),
      ],
      1.5,
    );

    expect(result).toHaveLength(3);
    expect(result[0].type).toBe('paragraph');
    expect(result[1]).toEqual({
      type: 'session-break',
      endLabel: 'End of previous recording',
      startLabel: 'New recording session started',
    });
    expect(result[2].type).toBe('paragraph');
  });

  it('inserts a live resume marker before appended live segments', () => {
    const segments = [
      seg('Saved session.', 0),
      seg('Live resumed session.', 0),
    ];

    const result = buildMeetingTranscriptBlocks(
      segments,
      [session('session-1', 0, 1)],
      1.5,
      1,
    );

    expect(result).toHaveLength(3);
    expect(result[1]).toEqual({
      type: 'session-break',
      endLabel: 'End of previous recording',
      startLabel: 'Resumed recording in progress',
    });
  });
});
