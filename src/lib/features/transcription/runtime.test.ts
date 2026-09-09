import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  getTranscriptionCatalog: vi.fn(),
  getModelStatus: vi.fn(),
  downloadModel: vi.fn(),
  loadModel: vi.fn(),
}));
vi.mock("../../api/transcription", () => api);

import {
  decideStartupModelAction,
  loadAfterOrphanedDownload,
  runStartupModelFlow,
  shouldLoadAfterOrphanedDownload,
  startTranscriptionModelDownload,
} from "./runtime";
import { getAppState } from "../../stores/app.svelte";
import { markSetupComplete } from "../onboarding/setup";
import { mockCatalog, mockRuntimeStatus, mockSettings } from "../../test-helpers/fixtures";
import type { AppStateMachine, DownloadProgress } from "../../types";

const profile = mockRuntimeStatus.profile;
const selection = {
  engine_id: profile.engine_id,
  model_id: profile.model_id,
  backend_id: profile.backend_id,
};
const downloading: AppStateMachine = { state: "downloading", data: { profile } };
const downloaded: AppStateMachine = { state: "downloaded", data: { profile } };

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

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

describe("shouldLoadAfterOrphanedDownload", () => {
  it("fires when a download completes into a dead channel", () => {
    expect(shouldLoadAfterOrphanedDownload("downloading", "downloaded", "load_required", false))
      .toBe(true);
  });

  it("leaves the load to the live channel callback", () => {
    expect(shouldLoadAfterOrphanedDownload("downloading", "downloaded", "load_required", true))
      .toBe(false);
  });

  it("ignores the downloaded state an idle-timeout unload lands on", () => {
    expect(shouldLoadAfterOrphanedDownload("unloading", "downloaded", "load_required", false))
      .toBe(false);
    expect(shouldLoadAfterOrphanedDownload("ready", "downloaded", "load_required", false))
      .toBe(false);
  });

  it("has no edge to fire on when the event repeats", () => {
    expect(shouldLoadAfterOrphanedDownload("downloaded", "downloaded", "load_required", false))
      .toBe(false);
  });

  it("does not load a model the user is no longer pointing at", () => {
    expect(shouldLoadAfterOrphanedDownload("downloading", "downloaded", "download_required", false))
      .toBe(false);
  });
});

describe("loadAfterOrphanedDownload", () => {
  const app = getAppState();

  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    app.settings = { ...mockSettings };
    app.machineState = { state: "idle" };
    api.loadModel.mockResolvedValue(undefined);
    api.getModelStatus.mockResolvedValue({ ...mockRuntimeStatus, phase: "ready" });
  });

  it("loads the model once when the DownloadComplete lands on a dead channel", async () => {
    // The StateChanged listener has already applied the new machine state,
    // which moved the selected profile's phase to load_required.
    app.machineState = downloaded;

    loadAfterOrphanedDownload(app, downloading, downloaded);
    await vi.waitFor(() => expect(api.loadModel).toHaveBeenCalledTimes(1));
    expect(api.loadModel).toHaveBeenCalledWith(selection);

    // A duplicate event has no edge left to fire on.
    loadAfterOrphanedDownload(app, downloaded, downloaded);
    await flush();
    expect(api.loadModel).toHaveBeenCalledTimes(1);
  });

  it("does not reload a model the idle timeout just unloaded", async () => {
    app.machineState = downloaded;

    loadAfterOrphanedDownload(app, { state: "unloading", data: { profile, next_profile: null } }, downloaded);
    await flush();

    expect(api.loadModel).not.toHaveBeenCalled();
  });

  it("defers to the live channel callback while this webview owns the download", async () => {
    // What get_model_status reports once the files are on disk.
    api.getModelStatus.mockResolvedValue({ ...mockRuntimeStatus, phase: "load_required" });
    let onProgress: ((progress: DownloadProgress) => void) | null = null;
    api.downloadModel.mockImplementation(async (_selection, callback) => {
      onProgress = callback;
    });
    await startTranscriptionModelDownload(app, null, () => {});
    expect(onProgress).not.toBeNull();

    app.machineState = downloaded;
    loadAfterOrphanedDownload(app, downloading, downloaded);
    await flush();
    expect(api.loadModel).not.toHaveBeenCalled();

    // The channel finishes (no autoLoad requested by this caller), which
    // hands the responsibility back to the safety net for the next download.
    onProgress!({
      file: "all",
      downloaded_bytes: 0,
      total_bytes: null,
      completed_files: 1,
      total_files: 1,
      status: "complete",
    });
    await flush();
    expect(api.loadModel).not.toHaveBeenCalled();

    loadAfterOrphanedDownload(app, downloading, downloaded);
    await vi.waitFor(() => expect(api.loadModel).toHaveBeenCalledTimes(1));
  });
});

describe("runStartupModelFlow", () => {
  const app = getAppState();

  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    app.settings = { ...mockSettings };
    app.machineState = { state: "idle" };
    app.showOnboarding = false;
    api.getTranscriptionCatalog.mockResolvedValue(mockCatalog);
    api.loadModel.mockResolvedValue(undefined);
  });

  it("loads a model the backend finished downloading while the webview was gone", async () => {
    markSetupComplete();
    app.machineState = downloaded;
    api.getModelStatus.mockResolvedValue({ ...mockRuntimeStatus, phase: "load_required" });

    await runStartupModelFlow(app);

    await vi.waitFor(() => expect(api.loadModel).toHaveBeenCalledTimes(1));
    expect(api.loadModel).toHaveBeenCalledWith(selection);
    expect(app.showOnboarding).toBe(false);
  });

  it("keeps the wizard closed on a reload mid-download once setup is done", async () => {
    markSetupComplete();
    app.machineState = downloading;
    api.getModelStatus.mockResolvedValue({ ...mockRuntimeStatus, phase: "download_required" });

    await runStartupModelFlow(app);

    expect(app.showOnboarding).toBe(false);
    expect(api.loadModel).not.toHaveBeenCalled();
  });
});
