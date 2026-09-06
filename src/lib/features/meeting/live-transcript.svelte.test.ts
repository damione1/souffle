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

/** Feed finalized segments one-by-one, mirroring the controller's index assignment. */
function feedSegments(live: ReturnType<typeof createLiveTranscript>, segments: TranscriptionSegment[]) {
  let segmentIndex = 0;
  for (const s of segments) {
    live.append(s, segmentIndex);
    if (s.is_final !== false) segmentIndex++;
  }
}

/** Feed segments one-by-one into a fresh grouper and return the final
 * flattened committed+tail paragraphs (stripped of live-only fields). */
function runStream(segments: TranscriptionSegment[]) {
  const live = createLiveTranscript(PAUSE_THRESHOLD);
  feedSegments(live, segments);
  const all = [...live.committed, ...live.tail].map(({ id: _id, segmentRange: _range, ...rest }) => rest);
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
    // ordered; the batch grouper merges lanes without time-sorting within one
    // speaker, then orders the resulting paragraphs by start.
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

  it("commits paragraphs once they fall outside the trailing time window", () => {
    // 2s gaps: pause break every sentence. Latest start is 12s; horizon is
    // 12 - 8 = 4s, so paragraphs ending before 4s freeze, later ones stay in
    // the tail (and the last paragraph is always kept open).
    const segments = Array.from({ length: 7 }, (_, i) =>
      seg(`Sentence number ${i + 1}.`, i * 2.0),
    );
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, segments);

    expect(live.committed.length).toBeGreaterThan(0);
    expect(live.tail.length).toBeGreaterThan(0);
    expect(live.committed.length + live.tail.length).toBe(7);
    expect(live.committed[live.committed.length - 1].startTime).toBeLessThan(
      live.tail[0].startTime,
    );
  });
});

describe("createLiveTranscript committed immutability", () => {
  it("never mutates a committed paragraph after it is committed", () => {
    const segments = Array.from({ length: 8 }, (_, i) =>
      seg(`Sentence number ${i + 1}.`, i * 2.0),
    );
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, segments);

    expect(live.committed.length).toBeGreaterThan(0);
    const snapshot = live.committed.map((p) => ({ ref: p, copy: { ...p } }));

    // Append more segments; already-committed paragraphs must stay identical.
    feedSegments(live, [
      seg("Ninth paragraph.", 16.0),
      seg("Tenth paragraph.", 18.0),
    ]);

    for (const { ref, copy } of snapshot) {
      expect(ref).toEqual(copy);
    }
  });
});

describe("createLiveTranscript tentative text", () => {
  it("sets tentative on a non-final segment and clears it on the next final", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("Hello wor", 0, { is_final: false }), 0);
    expect(live.tentative).toBe("Hello wor");
    expect(live.committed).toEqual([]);
    expect(live.tail).toEqual([]);

    live.append(seg("Hello world.", 0), 0);
    expect(live.tentative).toBe("");
    expect(live.tail).toHaveLength(1);
    expect(live.tail[0].text).toBe("Hello world.");
  });

  it("does not increment segmentCount for non-final segments", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("partial", 0, { is_final: false }), 0);
    expect(live.segmentCount).toBe(0);
    live.append(seg("partial done.", 0), 0);
    expect(live.segmentCount).toBe(1);
  });
});

describe("createLiveTranscript reset", () => {
  it("clears committed, tail, tentative, and segmentCount", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, [
      seg("First paragraph.", 0),
      seg("Second paragraph.", 2.0),
      seg("Third paragraph.", 4.0),
      seg("Fourth paragraph.", 6.0),
    ]);
    live.append(seg("partial", 8.0, { is_final: false }), 4);

    expect(live.committed.length + live.tail.length).toBeGreaterThan(0);
    expect(live.tentative).toBe("partial");

    live.reset();

    expect(live.committed).toEqual([]);
    expect(live.tail).toEqual([]);
    expect(live.tentative).toBe("");
    expect(live.segmentCount).toBe(0);

    // Grouper is fully usable again after reset.
    live.append(seg("Fresh start.", 0), 0);
    expect(live.tail).toHaveLength(1);
    expect(live.tail[0].text).toBe("Fresh start.");
  });
});

describe("createLiveTranscript paragraph ids", () => {
  it("assigns strictly increasing ids across committed and tail paragraphs", () => {
    const segments = Array.from({ length: 10 }, (_, i) => seg(`Sentence ${i}.`, i * 2.0));
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, segments);

    const ids = [...live.committed, ...live.tail].map((p) => p.id);
    expect(ids.length).toBeGreaterThan(1);
    for (let i = 1; i < ids.length; i++) {
      expect(ids[i]).toBeGreaterThan(ids[i - 1]);
    }
  });

  it("keeps the same id for a growing tail paragraph across appends", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    live.append(seg("Hello", 0), 0);
    expect(live.tail).toHaveLength(1);
    const firstId = live.tail[0].id;

    // No pause, no sentence end yet: still the same open paragraph.
    live.append(seg("world", 0.5), 1);
    expect(live.tail).toHaveLength(1);
    expect(live.tail[0].id).toBe(firstId);
    expect(live.tail[0].text).toBe("Hello world");
  });
});

describe("createLiveTranscript segment ranges and live edits", () => {
  it("tracks segment ranges on committed paragraphs", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, [
      seg("hello", 0),
      seg("world", 1),
      seg("again", 3),
    ]);

    const paragraphs = [...live.committed, ...live.tail];
    expect(paragraphs.length).toBeGreaterThan(0);
    expect(paragraphs[0].segmentRange.start).toBe(0);
    expect(paragraphs[0].segmentRange.end).toBeGreaterThan(0);
  });

  it("editParagraph updates committed text without resetting the stream", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, Array.from({ length: 8 }, (_, i) =>
      seg(`Sentence number ${i + 1}.`, i * 2.0),
    ));

    expect(live.committed.length).toBeGreaterThan(0);
    const target = live.committed[0];
    const updated = live.editParagraph(target.id, "hello universe");
    expect(updated?.text).toBe("hello universe");
    expect(live.committed[0].text).toBe("hello universe");
  });
});

describe("createLiveTranscript diarized tail window", () => {
  it("places a late Me segment by timestamp inside the still-open tail", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, [
      dseg("Them first.", 10.0, "them"),
      dseg("Them second.", 12.0, "them"),
    ]);
    live.append(dseg("Me interruption.", 11.0, "me"), 2);

    const all = [...live.committed, ...live.tail];
    expect(all.map((p) => ({ speaker: p.speaker, text: p.text }))).toEqual([
      { speaker: "them", text: "Them first." },
      { speaker: "me", text: "Me interruption." },
      { speaker: "them", text: "Them second." },
    ]);
  });

  it("does not split a lane into one paragraph per word when its clock regresses", () => {
    // Them keeps advancing; Me's lane clock restarts far behind it. With a
    // global horizon, every Me word lands under it, gets committed on its own,
    // and empties the tail for the next one.
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    const regressed = ["a", "b", "c", "d", "e", "f", "g"];
    feedSegments(live, [
      dseg("them one", 40.0, "them"),
      dseg("them two", 41.0, "them"),
      dseg("them three", 42.0, "them"),
      ...regressed.map((word, i) => dseg(word, 1.0 + i * 0.3, "me")),
    ]);

    const mine = [...live.committed, ...live.tail].filter((p) => p.speaker === "me");
    expect(mine).toHaveLength(1);
    expect(mine[0].text).toBe(regressed.join(" "));
  });

  it("does not let a silent lane pin the other lane's commits", () => {
    // Me said one thing, then went quiet. Them keeps talking well past the
    // tail window. Me's leftover paragraph must freeze so Them's older turns
    // can commit; a single slowest-lane horizon would hold everything at t=0.
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, [
      dseg("Me hello.", 0.0, "me"),
      dseg("Them one.", 20.0, "them"),
      dseg("Them two.", 22.0, "them"),
      dseg("Them three.", 24.0, "them"),
      dseg("Them four.", 26.0, "them"),
      dseg("Them five.", 28.0, "them"),
      dseg("Them six.", 30.0, "them"),
      dseg("Them seven.", 32.0, "them"),
    ]);

    expect(live.committed.map((p) => p.text)).toEqual([
      "Me hello.",
      "Them one.",
      "Them two.",
    ]);
    expect(live.tail[0]?.text).toBe("Them three.");
    expect(live.tail.map((p) => p.speaker)).toEqual([
      "them",
      "them",
      "them",
      "them",
      "them",
    ]);
  });

  it("does not zipper same-speaker words that arrive with overlapping timestamps", () => {
    const live = createLiveTranscript(PAUSE_THRESHOLD);
    feedSegments(live, [
      dseg("Je", 19.28, "them"),
      dseg("vous", 19.28, "them"),
      dseg("la", 19.30, "them"),
      dseg("mets", 19.29, "them"),
      dseg("dans", 19.32, "them"),
      dseg("le", 19.31, "them"),
      dseg("chat", 19.34, "them"),
    ]);

    const all = [...live.committed, ...live.tail];
    expect(all).toHaveLength(1);
    expect(all[0].text).toBe("Je vous la mets dans le chat");
  });
});
