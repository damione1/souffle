import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";

const { transcriptionApi, settingsApi, permissionsApi } = vi.hoisted(() => ({
  transcriptionApi: {
    getTranscriptionCatalog: vi.fn(),
  },
  settingsApi: {
    getSettings: vi.fn(),
    saveSettings: vi.fn(),
    getShortcuts: vi.fn(),
    saveShortcuts: vi.fn(),
    listAudioDevices: vi.fn(),
    selectAudioDevice: vi.fn(),
  },
  permissionsApi: {
    getPermissionStatus: vi.fn(),
    requestPermission: vi.fn(),
    repairAccessibilityPermission: vi.fn(),
  },
}));

vi.mock("../../api/transcription", () => transcriptionApi);
vi.mock("../../api/settings", () => settingsApi);
vi.mock("../../api/permissions", () => permissionsApi);

import OnboardingView from "./OnboardingView.svelte";
import { getAppState } from "../../stores/app.svelte";
import { mockCatalog, mockShortcuts } from "../../test-helpers/fixtures";
import { PERMISSIONS_STORAGE_KEY } from "./setup";

const fakeDevices = [
  { uid: "builtin-mic", name: "Built-in Microphone", transport: "built_in" as const, is_default: true },
];

describe("OnboardingView shortcut step (SOU-053)", () => {
  const app = getAppState();

  beforeEach(() => {
    localStorage.clear();
    // Skip the permissions step: the wizard must still know the real
    // Accessibility state through the mount-time probe, not just through
    // PermissionsStep being on screen.
    localStorage.setItem(PERMISSIONS_STORAGE_KEY, "1");
    app.showOnboarding = true;
    app.machineState = { state: "idle" };
    app.transcriptionRuntimePhase = "ready";
    app.selectedDevice = "";

    transcriptionApi.getTranscriptionCatalog.mockResolvedValue(mockCatalog);
    settingsApi.getShortcuts.mockResolvedValue({ ...mockShortcuts });
    settingsApi.saveShortcuts.mockResolvedValue(undefined);
    settingsApi.saveSettings.mockResolvedValue(undefined);
    settingsApi.listAudioDevices.mockResolvedValue(fakeDevices);
    settingsApi.selectAudioDevice.mockResolvedValue(undefined);
  });

  afterEach(cleanup);

  async function goToShortcutStep() {
    render(OnboardingView);
    await waitFor(() => screen.getByText("Choose your microphone"));
    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Download the transcription model"));
    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Quick dictation shortcut"));
  }

  it("shows an actionable warning under auto-paste when Accessibility is missing", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue({
      microphone: "granted",
      system_audio: "granted",
      accessibility: "denied",
      calendar: "unknown",
    });

    await goToShortcutStep();

    expect(screen.getByTestId("auto-paste-accessibility-warning")).toBeTruthy();
    expect(screen.getByTestId("review-accessibility")).toBeTruthy();
  });

  it("hides the warning once Accessibility is granted", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue({
      microphone: "granted",
      system_audio: "granted",
      accessibility: "granted",
      calendar: "unknown",
    });

    await goToShortcutStep();

    expect(screen.queryByTestId("auto-paste-accessibility-warning")).toBeNull();
  });
});

// SOU-130: the dictionary interview used to add a fifth step after the
// shortcut. The rendered progress and the final button label must follow the
// four real steps, not a count the view carries separately.
describe("OnboardingView step count (SOU-130)", () => {
  const app = getAppState();

  beforeEach(() => {
    localStorage.clear();
    app.showOnboarding = true;
    app.machineState = { state: "idle" };
    app.transcriptionRuntimePhase = "ready";
    app.selectedDevice = "";

    transcriptionApi.getTranscriptionCatalog.mockResolvedValue(mockCatalog);
    settingsApi.getShortcuts.mockResolvedValue({ ...mockShortcuts });
    settingsApi.saveShortcuts.mockResolvedValue(undefined);
    settingsApi.saveSettings.mockResolvedValue(undefined);
    settingsApi.listAudioDevices.mockResolvedValue(fakeDevices);
    settingsApi.selectAudioDevice.mockResolvedValue(undefined);
    permissionsApi.getPermissionStatus.mockResolvedValue({
      microphone: "granted",
      system_audio: "granted",
      accessibility: "granted",
      calendar: "unknown",
    });
  });

  afterEach(cleanup);

  it("walks four steps and ends on the shortcut with the finish label", async () => {
    render(OnboardingView);

    await waitFor(() => screen.getByText("Grant permissions"));
    const progress = screen.getByRole("progressbar");
    expect(progress.getAttribute("aria-valuemax")).toBe("4");
    expect(progress.children).toHaveLength(4);
    expect(progress.getAttribute("aria-valuenow")).toBe("1");

    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Choose your microphone"));

    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Download the transcription model"));

    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Quick dictation shortcut"));

    // Last step: the progress is full and the primary button offers to finish.
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("4");
    expect(screen.getByRole("button", { name: "Get started" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
  });

  it("closes the wizard from the shortcut step", async () => {
    render(OnboardingView);

    await waitFor(() => screen.getByText("Grant permissions"));
    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Choose your microphone"));
    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Download the transcription model"));
    await fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => screen.getByText("Quick dictation shortcut"));

    await fireEvent.click(screen.getByRole("button", { name: "Get started" }));

    await waitFor(() => expect(app.showOnboarding).toBe(false));
  });
});
