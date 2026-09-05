import { describe, it, expect } from 'vitest';
import { elapsedSecondsSince } from './elapsed';

describe('elapsedSecondsSince', () => {
  const anchor = Date.UTC(2026, 0, 1, 9, 0, 0);

  it('is zero without an anchor', () => {
    expect(elapsedSecondsSince(null, anchor + 60_000)).toBe(0);
  });

  it('is zero at the anchor', () => {
    expect(elapsedSecondsSince(anchor, anchor)).toBe(0);
  });

  it('truncates sub-second remainders', () => {
    expect(elapsedSecondsSince(anchor, anchor + 1_900)).toBe(1);
  });

  /** The regression: no matter how few repaints happened in between, the
   * elapsed value comes from the clock gap, not from a tick count. */
  it('reports the full gap across a long unobserved stretch', () => {
    expect(elapsedSecondsSince(anchor, anchor + 45 * 60_000)).toBe(2700);
  });

  it('never goes negative when the clock moves backwards', () => {
    expect(elapsedSecondsSince(anchor, anchor - 5_000)).toBe(0);
  });
});
