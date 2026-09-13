import { describe, expect, it } from "vitest";
import { minuteOptions } from "./minute-options";

const LABELS = { 5: "five", 10: "ten" };

describe("minuteOptions", () => {
  it("keeps the backend's values and order", () => {
    expect(minuteOptions([10, 5], LABELS)).toEqual([
      { value: 10, labelKey: "ten" },
      { value: 5, labelKey: "five" },
    ]);
  });

  it("renders an unlabelled backend value instead of dropping it", () => {
    expect(minuteOptions([5, 42], LABELS)).toEqual([
      { value: 5, labelKey: "five" },
      { value: 42, labelKey: null },
    ]);
  });

  it("offers nothing before the backend list has arrived", () => {
    expect(minuteOptions(undefined, LABELS)).toEqual([]);
  });
});
