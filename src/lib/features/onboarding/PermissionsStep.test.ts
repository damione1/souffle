import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import type { PermissionStatus, PermState } from "../../types";

const { permissionsApi } = vi.hoisted(() => ({
  permissionsApi: {
    getPermissionStatus: vi.fn(),
    requestPermission: vi.fn(),
    repairAccessibilityPermission: vi.fn(),
  },
}));

vi.mock("../../api/permissions", () => permissionsApi);

import PermissionsStep from "./PermissionsStep.svelte";

function statusWith(microphone: PermState): PermissionStatus {
  return {
    microphone,
    system_audio: "unknown",
    accessibility: "granted",
    calendar: "unknown",
  };
}

function rowFor(label: string): HTMLElement {
  const row = screen.getByText(label).closest(".rounded-lg");
  if (!(row instanceof HTMLElement)) {
    throw new Error(`row not found for label "${label}"`);
  }
  return row;
}

describe("PermissionsStep microphone denial", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows the denied hint and an Open Settings button under the mic row", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(statusWith("denied"));
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());

    const micRow = rowFor("Microphone");
    expect(within(micRow).getByText(/won't ask again/)).toBeTruthy();
    expect(within(micRow).getByRole("button", { name: "Open Settings" })).toBeTruthy();
  });

  it("shows a distinct hint (no button) when there is no input device", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(statusWith("no_device"));
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());

    const micRow = rowFor("Microphone");
    expect(within(micRow).getByText(/No microphone was found/)).toBeTruthy();
    expect(within(micRow).queryByRole("button", { name: "Open Settings" })).toBeNull();
  });

  it("does not show the denied hint for an unrelated state", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(statusWith("unknown"));
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());

    const micRow = rowFor("Microphone");
    expect(within(micRow).queryByText(/won't ask again/)).toBeNull();
    expect(within(micRow).queryByText(/No microphone was found/)).toBeNull();
  });
});

describe("PermissionsStep accessibility repair", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  function accessibilityDenied(): PermissionStatus {
    return {
      microphone: "granted",
      system_audio: "unknown",
      accessibility: "denied",
      calendar: "unknown",
    };
  }

  /** Repair is only offered once the stale-entry diagnosis applies: the user
   * clicked Open Settings, left, and came back with it still refused. */
  async function reachRepair(): Promise<HTMLElement> {
    const axRow = rowFor("Accessibility");
    permissionsApi.requestPermission.mockResolvedValue("denied");
    await fireEvent.click(within(axRow).getByRole("button", { name: "Open Settings" }));
    await fireEvent.blur(window);
    await fireEvent.focus(window);
    await waitFor(() =>
      expect(within(axRow).getByRole("button", { name: "Repair permission" })).toBeTruthy(),
    );
    return axRow;
  }

  it("announces success when tccutil reset succeeds", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    permissionsApi.repairAccessibilityPermission.mockResolvedValue({
      reset_performed: true,
      prompt_shown: true,
    });
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());
    const axRow = await reachRepair();
    await fireEvent.click(within(axRow).getByRole("button", { name: "Repair permission" }));

    await waitFor(() => {
      expect(within(axRow).getByText(/new prompt should appear/i)).toBeTruthy();
    });
    expect(permissionsApi.repairAccessibilityPermission).toHaveBeenCalledOnce();
  });

  it("does not claim success when the repair command fails", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    permissionsApi.repairAccessibilityPermission.mockRejectedValue("tccutil reset Accessibility failed (exit 64)");
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());
    const axRow = await reachRepair();
    await fireEvent.click(within(axRow).getByRole("button", { name: "Repair permission" }));

    await waitFor(() => {
      expect(screen.getByText(/tccutil reset Accessibility failed/)).toBeTruthy();
    });
    expect(within(axRow).queryByText(/new prompt should appear/i)).toBeNull();
    expect(within(axRow).getByText(/stale entry/i)).toBeTruthy();
  });
});

describe("PermissionsStep accessibility on a fresh install (SOU-055)", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  function accessibilityDenied(): PermissionStatus {
    return {
      microphone: "granted",
      system_audio: "unknown",
      accessibility: "denied",
      calendar: "unknown",
    };
  }

  it("does not diagnose a stale TCC entry before the user has tried anything", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());

    const axRow = rowFor("Accessibility");
    expect(within(axRow).queryByText(/stale entry/i)).toBeNull();
    expect(within(axRow).queryByRole("button", { name: "Repair permission" })).toBeNull();
    expect(within(axRow).getByText(/tick Soufflé in the Accessibility list/i)).toBeTruthy();
  });

  it("still does not diagnose it right after Open Settings, before the user comes back", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    // request_permission answers Denied synchronously while System Settings
    // is still opening; that answer says nothing about the user's intent.
    permissionsApi.requestPermission.mockResolvedValue("denied");
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());
    const axRow = rowFor("Accessibility");
    await fireEvent.click(within(axRow).getByRole("button", { name: "Open Settings" }));

    await waitFor(() => expect(permissionsApi.requestPermission).toHaveBeenCalledWith("accessibility"));
    expect(within(axRow).queryByText(/stale entry/i)).toBeNull();
    expect(within(axRow).queryByRole("button", { name: "Repair permission" })).toBeNull();
  });

  it("diagnoses it once the user has left for Settings and come back still refused", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    permissionsApi.requestPermission.mockResolvedValue("denied");
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());
    const axRow = rowFor("Accessibility");
    await fireEvent.click(within(axRow).getByRole("button", { name: "Open Settings" }));
    await fireEvent.blur(window);
    await fireEvent.focus(window);

    await waitFor(() => {
      expect(within(axRow).getByText(/stale entry/i)).toBeTruthy();
    });
    expect(within(axRow).getByRole("button", { name: "Repair permission" })).toBeTruthy();
  });

  it("diagnoses it on a second attempt even without a blur/focus pair", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    permissionsApi.requestPermission.mockResolvedValue("denied");
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());
    const axRow = rowFor("Accessibility");
    const openSettings = () =>
      fireEvent.click(within(axRow).getByRole("button", { name: "Open Settings" }));

    await openSettings();
    expect(within(axRow).queryByText(/stale entry/i)).toBeNull();
    await openSettings();

    await waitFor(() => {
      expect(within(axRow).getByText(/stale entry/i)).toBeTruthy();
    });
  });

  it("drops the diagnosis again once the permission is granted", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(accessibilityDenied());
    permissionsApi.requestPermission.mockResolvedValue("denied");
    render(PermissionsStep);

    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());
    const axRow = rowFor("Accessibility");
    await fireEvent.click(within(axRow).getByRole("button", { name: "Open Settings" }));
    await fireEvent.blur(window);
    await fireEvent.focus(window);
    await waitFor(() => expect(within(axRow).getByText(/stale entry/i)).toBeTruthy());

    // The 600 ms poll reports the grant.
    permissionsApi.getPermissionStatus.mockResolvedValue({
      ...accessibilityDenied(),
      accessibility: "granted",
    });
    await waitFor(
      () => {
        expect(within(axRow).getByText("Granted")).toBeTruthy();
      },
      { timeout: 3000 },
    );
    expect(within(axRow).queryByText(/stale entry/i)).toBeNull();
  });
});

describe("PermissionsStep per-row busy state", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("only disables the row being probed, not the other permission buttons", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(statusWith("unknown"));
    let resolveRequest: (value: PermState) => void = () => {};
    permissionsApi.requestPermission.mockImplementation(
      () =>
        new Promise<PermState>((resolve) => {
          resolveRequest = resolve;
        }),
    );

    render(PermissionsStep);
    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());

    const micButton = within(rowFor("Microphone")).getByRole("button") as HTMLButtonElement;
    const systemAudioButton = within(rowFor("System audio")).getByRole(
      "button",
    ) as HTMLButtonElement;

    await fireEvent.click(micButton);

    expect(permissionsApi.requestPermission).toHaveBeenCalledWith("microphone");
    expect(micButton.disabled).toBe(true);
    expect(systemAudioButton.disabled).toBe(false);

    resolveRequest("granted");
  });

  it("handles two concurrent pending requests remaining independently active until each completes", async () => {
    permissionsApi.getPermissionStatus.mockResolvedValue(statusWith("unknown"));
    let resolveMic: (value: PermState) => void = () => {};
    let resolveSys: (value: PermState) => void = () => {};
    
    permissionsApi.requestPermission.mockImplementation((kind) => {
      return new Promise<PermState>((resolve) => {
        if (kind === "microphone") {
          resolveMic = resolve;
        } else if (kind === "system_audio") {
          resolveSys = resolve;
        }
      });
    });

    render(PermissionsStep);
    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalled());

    const micButton = within(rowFor("Microphone")).getByRole("button") as HTMLButtonElement;
    const systemAudioButton = within(rowFor("System audio")).getByRole("button") as HTMLButtonElement;

    await fireEvent.click(micButton);
    await fireEvent.click(systemAudioButton);

    expect(micButton.disabled).toBe(true);
    expect(systemAudioButton.disabled).toBe(true);
    
    // Resolve microphone to "unknown" so the button stays in the DOM, 
    // allowing us to verify its disabled state transitions back to false.
    resolveMic("unknown");
    
    // Using await waitFor to ensure svelte state reactivity
    await waitFor(() => {
      expect(micButton.disabled).toBe(false);
    });
    // System audio should still be disabled
    expect(systemAudioButton.disabled).toBe(true);
    
    // Resolve system audio
    resolveSys("unknown");
    
    await waitFor(() => {
      expect(systemAudioButton.disabled).toBe(false);
    });
  });
});

describe("PermissionsStep permission poll", () => {
  afterEach(() => {
    vi.useRealTimers();
    cleanup();
    vi.clearAllMocks();
  });

  it("skips a poll tick while the previous request is in flight", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
    let resolvePoll: (value: PermissionStatus) => void = () => {};
    permissionsApi.getPermissionStatus
      .mockResolvedValueOnce(statusWith("denied"))
      .mockImplementationOnce(
        () =>
          new Promise<PermissionStatus>((resolve) => {
            resolvePoll = resolve;
          }),
      )
      .mockResolvedValue(statusWith("granted"));

    render(PermissionsStep);
    await waitFor(() => expect(permissionsApi.getPermissionStatus).toHaveBeenCalledTimes(1));

    await vi.advanceTimersByTimeAsync(600);
    expect(permissionsApi.getPermissionStatus).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(600);
    expect(permissionsApi.getPermissionStatus).toHaveBeenCalledTimes(2);

    resolvePoll(statusWith("granted"));
    await waitFor(() => {
      expect(within(rowFor("Microphone")).getByText("Granted")).toBeTruthy();
    });

    await vi.advanceTimersByTimeAsync(600);
    expect(permissionsApi.getPermissionStatus).toHaveBeenCalledTimes(3);
  });
});
