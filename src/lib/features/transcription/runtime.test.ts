import { describe, expect, it } from "vitest";
import { decideStartupModelAction } from "./runtime";

describe("decideStartupModelAction", () => {
  it("shows onboarding when no model is downloaded", () => {
    expect(decideStartupModelAction("download_required", "idle")).toBe("onboarding");
    expect(decideStartupModelAction("download_required", "ready")).toBe("onboarding");
  });

  it("auto-loads a downloaded model from a settled cold state", () => {
    expect(decideStartupModelAction("load_required", "idle")).toBe("load");
    expect(decideStartupModelAction("load_required", "downloaded")).toBe("load");
  });

  it("does nothing on a webview reload while the backend is busy or ready", () => {
    expect(decideStartupModelAction("load_required", "loading")).toBe("none");
    expect(decideStartupModelAction("load_required", "ready")).toBe("none");
    expect(decideStartupModelAction("load_required", "recording_dictation")).toBe("none");
    expect(decideStartupModelAction("ready", "ready")).toBe("none");
    expect(decideStartupModelAction("ready", "idle")).toBe("none");
  });
});
