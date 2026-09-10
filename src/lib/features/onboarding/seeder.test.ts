import { describe, expect, it, vi } from "vitest";
import { seedDictionary } from "./seeder";
import * as api from "../../api/dictionary";

describe("seedDictionary", () => {
  it("processes all terms even if one throws an error", async () => {
    const addSpy = vi.spyOn(api, "addDictionaryEntry").mockImplementation(async (term) => {
      if (term === "fail") throw new Error("Duplicate");
      return {} as any;
    });

    await seedDictionary("ok1, fail, ok2; ok3");

    expect(addSpy).toHaveBeenCalledTimes(4);
    expect(addSpy).toHaveBeenNthCalledWith(1, "ok1", null, "interview");
    expect(addSpy).toHaveBeenNthCalledWith(2, "fail", null, "interview");
    expect(addSpy).toHaveBeenNthCalledWith(3, "ok2", null, "interview");
    expect(addSpy).toHaveBeenNthCalledWith(4, "ok3", null, "interview");

    addSpy.mockRestore();
  });
});
