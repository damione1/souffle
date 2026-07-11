import { describe, it, expect } from 'vitest';
import { buildMeetingTranscriptBlocks, groupIntoParagraphs, groupIntoParagraphsWithRanges } from './paragraphs';
import type { MeetingRecordingSession, Speaker, TranscriptionSegment } from '../types';
import paragraphGroupingFixture from '../test-helpers/paragraph-grouping.fixture.json';

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

  it('keeps each speaker flowing as one paragraph during word-level crosstalk', () => {
    // Kyutai-style one-segment-per-word streams, Me and Them talking at the
    // same time: interleaved by start_time this would otherwise flush a new
    // paragraph on every single word.
    const segments = [
      dseg('hello', 0.0, 'me'),
      dseg('hi', 0.15, 'them'),
      dseg('how', 0.5, 'me'),
      dseg('good', 0.6, 'them'),
      dseg('are', 0.9, 'me'),
      dseg('thanks', 1.0, 'them'),
      dseg('you', 1.3, 'me'),
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(2);
    expect(result[0]).toMatchObject({ speaker: 'me', text: 'hello how are you' });
    expect(result[1]).toMatchObject({ speaker: 'them', text: 'hi good thanks' });
  });

  it('closes a turn and starts a new one for the same speaker after a real pause', () => {
    // Me speaks, pauses well past the threshold, then speaks again: even
    // with no other speaker in between, this must not merge into one turn.
    const segments = [
      dseg('first turn.', 0, 'me'),
      dseg('second turn.', 5, 'me'),
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result).toHaveLength(2);
    expect(result[0].text).toBe('first turn.');
    expect(result[1].text).toBe('second turn.');
  });

  it('renders normal turn-taking unchanged (each pause closes the speaking turn)', () => {
    const segments = [
      dseg('Hi, how are you?', 0, 'me'),
      dseg("I'm good, thanks.", 3, 'them'),
      dseg('Great to hear.', 6, 'me'),
    ];
    const result = groupIntoParagraphs(segments, 1.5);
    expect(result.map((p) => ({ speaker: p.speaker, text: p.text }))).toEqual([
      { speaker: 'me', text: 'Hi, how are you?' },
      { speaker: 'them', text: "I'm good, thanks." },
      { speaker: 'me', text: 'Great to hear.' },
    ]);
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

describe('groupIntoParagraphsWithRanges', () => {
  function checkRangesReconstructParagraphs(segments: TranscriptionSegment[], pauseThreshold: number) {
    const { paragraphs, ranges, ordered } = groupIntoParagraphsWithRanges(segments, pauseThreshold);
    const batch = groupIntoParagraphs(segments, pauseThreshold);
    expect(paragraphs).toEqual(batch);
    expect(ranges).toHaveLength(paragraphs.length);

    // Ranges form a contiguous, non-overlapping, ascending partition.
    let expectedStart = 0;
    for (const range of ranges) {
      expect(range.start).toBe(expectedStart);
      expect(range.end).toBeGreaterThan(range.start);
      expectedStart = range.end;
    }

    // Reconstructing each paragraph's text from its range (skipping empty
    // segments, same as the grouping rules) must reproduce the paragraph.
    for (let i = 0; i < ranges.length; i++) {
      const { start, end } = ranges[i];
      const reconstructedText = ordered
        .slice(start, end)
        .map((s) => s.text.trim())
        .filter((t) => t.length > 0)
        .join(' ');
      expect(reconstructedText).toBe(paragraphs[i].text);
    }
  }

  it('covers a plain in-order stream with contiguous ranges', () => {
    const segments = [
      seg('Hello world.', 0),
      seg('New paragraph after a pause.', 2.0),
      seg('And more.', 2.5),
    ];
    checkRangesReconstructParagraphs(segments, 1.5);
  });

  it('covers a diarized, interleaved stream with contiguous ranges', () => {
    const segments = [
      dseg('second', 3, 'them'),
      dseg('first', 1, 'me'),
      dseg('third', 5, 'them'),
    ];
    checkRangesReconstructParagraphs(segments, 1.5);
  });

  it('reorders crosstalk into per-speaker turns in `ordered`, with ranges over that reordering', () => {
    const segments = [
      dseg('hello', 0.0, 'me'),
      dseg('hi', 0.15, 'them'),
      dseg('how', 0.5, 'me'),
      dseg('good', 0.6, 'them'),
      dseg('are', 0.9, 'me'),
      dseg('thanks', 1.0, 'them'),
      dseg('you', 1.3, 'me'),
    ];
    const { ordered, ranges, paragraphs } = groupIntoParagraphsWithRanges(segments, 1.5);

    // `ordered` groups Me's turn contiguously, then Them's turn, not the
    // original interleaved-by-time order.
    expect(ordered.map((s) => s.text)).toEqual(['hello', 'how', 'are', 'you', 'hi', 'good', 'thanks']);
    expect(ranges).toEqual([{ start: 0, end: 4 }, { start: 4, end: 7 }]);
    expect(paragraphs.map((p) => p.text)).toEqual(['hello how are you', 'hi good thanks']);

    checkRangesReconstructParagraphs(segments, 1.5);
  });

  it('keeps an empty-text segment inside the range of the paragraph being built', () => {
    const segments = [
      seg('Hello', 0),
      seg('', 0.3), // empty text, mid-paragraph
      seg('world.', 0.6),
    ];
    const { paragraphs, ranges, ordered } = groupIntoParagraphsWithRanges(segments, 1.5);
    expect(paragraphs).toHaveLength(1);
    expect(paragraphs[0].text).toBe('Hello world.');
    expect(ranges).toEqual([{ start: 0, end: 3 }]);
    expect(ordered).toHaveLength(3);
  });

  it('returns empty ranges for empty input', () => {
    expect(groupIntoParagraphsWithRanges([], 1.5)).toEqual({ paragraphs: [], ranges: [], ordered: [] });
  });

  it('matches groupIntoParagraphs output for a long unpunctuated stream', () => {
    const segments = Array.from({ length: 300 }, (_, i) => seg(`word${i}`, i * 0.1));
    checkRangesReconstructParagraphs(segments, 1.5);
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

// Cross-language fixture shared with the Rust port in src-tauri/src/export.rs
// (tests/fixtures/paragraph_grouping.json). Both implementations must group
// the same segment streams into the same paragraphs — this is the frozen
// contract; if groupIntoParagraphs' behavior changes on purpose, regenerate
// and update both copies of the fixture together.
describe('groupIntoParagraphs cross-language fixture parity', () => {
  const { pause_threshold, cases } = paragraphGroupingFixture;

  for (const testCase of cases) {
    it(`matches the frozen fixture for "${testCase.name}"`, () => {
      const segments: TranscriptionSegment[] = testCase.segments.map((s) => ({
        text: s.text,
        start_time: s.start_time,
        end_time: s.end_time,
        is_final: true,
        language: null,
        confidence: null,
        speaker: (s.speaker ?? undefined) as Speaker | undefined,
      }));

      const result = groupIntoParagraphs(segments, pause_threshold);

      expect(result.map((p) => ({ timestamp: p.timestamp, text: p.text, speaker: p.speaker ?? null })))
        .toEqual(testCase.expected);
    });
  }
});
