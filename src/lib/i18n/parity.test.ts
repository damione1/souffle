import { describe, expect, it } from "vitest";
import en from "./en.json";
import fr from "./fr.json";

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(obj).flatMap(([key, value]) =>
    value !== null && typeof value === "object"
      ? flattenKeys(value as Record<string, unknown>, `${prefix}${key}.`)
      : [`${prefix}${key}`],
  );
}

describe("i18n locales", () => {
  it("en and fr expose exactly the same keys", () => {
    expect(flattenKeys(fr).sort()).toEqual(flattenKeys(en).sort());
  });
});
