import { describe, it, expect } from "vitest";
import { createLiveTranscript } from "./live-transcript.svelte";
import { groupIntoParagraphs } from "../../utils/paragraphs";
import type { Speaker, TranscriptionSegment } from "../../types";

function seg(
  text: string,
  start: number,
  overrides: Partial<TranscriptionSegment> = {},
): TranscriptionSegment {
  return {
    text,
    start_time: start,
    end_time: start + 0.5,
    is_final: true,
    language: null,
    confidence: null,
    ...overrides,
  };
}

function dseg(text: string, start: number, speaker: Speaker): TranscriptionSegment {
  return seg(text, start, { speaker });
}

const PAUSE_THRESHOLD = 1.5;

/** Feed segments one-by-one into a fresh grouper and return the final
 * flattened committed+tail paragraphs (stripped of the live-only `id`). */
function runStream(segments: TranscriptionSegment[]) {
  const live = createLiveTranscript(PAUSE_THRESHOLD);
  for (const s of segments) live.append(s);
  const all = [...live.committed, ...live.tail].map(({ id: _id, ...rest }) => rest);
  return { live, all };
}

describe("createLiveTranscript equivalence", () => {
  it("matches batch grouping for a plain in-order stream", () => {
    const segments = [
      seg("Hello world.", 0),
      seg("New paragraph after a pause.", 2.0),
      seg("And more.", 2.5),
      seg("Yet another sentence.", 3.0),
    ];
    const { all } = runStream(segments);
    expect(all).toEqual(groupIntoParagraphs(segments, PAUSE_THRESHOLD));
  });

  it("matches batch grouping for a heavily punctuated stream that spans many paragraphs", () => {
    const segments = Array.from({ length: 20 }, (_, i) =>
      seg(`Sentence number ${i + 1}.`, i * 2.0), // 2s gaps: pause break every sentence
    );
    const { all } = runStream(segments);
    expect(all).toEqual(groupIntoParagraphs(segments, PAUSE_THRESHOLD));
  });

  it("matches batch grouping for diarized alternating speakers with slight interleave", () => {
    // Mic and system audio lanes arrive close together but not perfectly
    // ordered; the grouper still sorts by start_time before grouping.
    const segments = [
      dseg("hi there", 0, "me"),
      dseg("hello back", 2.1, "them"),
      dseg("how are you", 4.3, "me"),
      dseg("great thanks", 4.35, "them"), // arrives right after, slightly interleaved
      dseg("good to hear", 6.5, "me"),
    ];
    const { all } = runStream(segments);
    expect(all).toEqual(groupIntoParagraphs(segments, PAUSE_THRESHOLD));
  });

  it("matches batch grouping for a long unpunctuated stream (hard length cap)", () => {
    const segments = Array.from({ length: 300 }, (_, i) => seg(`word${i}`, i * 0.1));
    const { all } = runStream(segments);
    expect(all).toEqual(groupIntoParagraphs(segments, PAUSE_THRESHOLD));
  });

  it("commits paragraphs to `committed` once more than 2 paragraphs are open", () => {
    const segments = [
      seg("First paragraph.", 0),
      seg("Second paragraph.", 2.0),
      seg("Third paragraph.", 4.0),
      seg("Fourth paragraph.", 6.0),
    ];
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    for (const s of segments) live.append(s);

    expect(live.tail.length).toBeLessThanOrEqual(2);
    expect(live.committed.length + live.tail.length).toBe(4);
    expect(live.committed.length).toBeGreaterThan(0);
  });
});

describe("createLiveTranscript committed immutability", () => {
  it("never mutates a committed paragraph after it is committed", () => {
    const segments = [
      seg("First paragraph.", 0),
      seg("Second paragraph.", 2.0),
      seg("Third paragraph.", 4.0),
      seg("Fourth paragraph.", 6.0),
      seg("Fifth paragraph.", 8.0),
    ];
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    for (const s of segments) live.append(s);

    expect(live.committed.length).toBeGreaterThan(0);
    const snapshot = live.committed.map((p) => ({ ref: p, copy: { ...p } }));

    // Append more segments; already-committed paragraphs must stay identical.
    live.append(seg("Sixth paragraph.", 10.0));
    live.append(seg("Seventh paragraph.", 12.0));

    for (const { ref, copy } of snapshot) {
      expect(ref).toEqual(copy);
    }
  });
});

describe("createLiveTranscript tentative text", () => {
  it("sets tentative on a non-final segment and clears it on the next final", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("Hello wor", 0, { is_final: false }));
    expect(live.tentative).toBe("Hello wor");
    expect(live.committed).toEqual([]);
    expect(live.tail).toEqual([]);

    live.append(seg("Hello world.", 0));
    expect(live.tentative).toBe("");
    expect(live.tail).toHaveLength(1);
    expect(live.tail[0].text).toBe("Hello world.");
  });

  it("does not increment segmentCount for non-final segments", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("partial", 0, { is_final: false }));
    expect(live.segmentCount).toBe(0);
    live.append(seg("partial done.", 0));
    expect(live.segmentCount).toBe(1);
  });
});

describe("createLiveTranscript reset", () => {
  it("clears committed, tail, tentative, and segmentCount", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("First paragraph.", 0));
    live.append(seg("Second paragraph.", 2.0));
    live.append(seg("Third paragraph.", 4.0));
    live.append(seg("Fourth paragraph.", 6.0));
    live.append(seg("partial", 8.0, { is_final: false }));

    expect(live.committed.length + live.tail.length).toBeGreaterThan(0);
    expect(live.tentative).toBe("partial");

    live.reset();

    expect(live.committed).toEqual([]);
    expect(live.tail).toEqual([]);
    expect(live.tentative).toBe("");
    expect(live.segmentCount).toBe(0);

    // Grouper is fully usable again after reset.
    live.append(seg("Fresh start.", 0));
    expect(live.tail).toHaveLength(1);
    expect(live.tail[0].text).toBe("Fresh start.");
  });
});

describe("createLiveTranscript paragraph ids", () => {
  it("assigns strictly increasing ids across committed and tail paragraphs", () => {
    const segments = Array.from({ length: 10 }, (_, i) => seg(`Sentence ${i}.`, i * 2.0));
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    for (const s of segments) live.append(s);

    const ids = [...live.committed, ...live.tail].map((p) => p.id);
    expect(ids.length).toBeGreaterThan(1);
    for (let i = 1; i < ids.length; i++) {
      expect(ids[i]).toBeGreaterThan(ids[i - 1]);
    }
  });

  it("keeps the same id for a growing tail paragraph across appends", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("Hello", 0));
    expect(live.tail).toHaveLength(1);
    const firstId = live.tail[0].id;

    // No pause, no sentence end yet: still the same open paragraph.
    live.append(seg("world", 0.5));
    expect(live.tail).toHaveLength(1);
    expect(live.tail[0].id).toBe(firstId);
    expect(live.tail[0].text).toBe("Hello world");
  });
});
