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
});
