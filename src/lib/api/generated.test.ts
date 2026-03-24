import { describe, expect, it } from "vitest";
import { unwrap } from "./generated";

describe("unwrap", () => {
  it("returns data from successful command results", async () => {
    await expect(
      unwrap(Promise.resolve({ status: "ok", data: "ready" } as const)),
    ).resolves.toBe("ready");
  });

  it("throws the backend error for failed command results", async () => {
    await expect(
      unwrap(Promise.resolve({ status: "error", error: "boom" } as const)),
    ).rejects.toBe("boom");
  });
});
