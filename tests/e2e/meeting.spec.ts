import { expect, test } from "@playwright/test";
import { mockRuntimeStatus } from "../../src/lib/test-helpers/fixtures";
import { emitTauriEvent, installTauriStub, sendChannelMessage, stubbedCalls } from "./tauri-stub";

const PROFILE = mockRuntimeStatus.profile;

test("starting and stopping a meeting recording returns the UI to a non-recording state", async ({ page }) => {
  await installTauriStub(page);
  await page.goto("/");

  const meetingButton = page.getByRole("button", { name: /^Meeting\b/ });
  await expect(meetingButton).toBeVisible();
  await meetingButton.click();

  // `start_meeting_recording` resolves (stubbed to succeed) but nothing
  // flips the store to "recording" until the backend's `state-changed`
  // event arrives — simulate that the way the real backend would.
  await expect
    .poll(async () => (await stubbedCalls(page)).some((call) => call.cmd === "start_meeting_recording"))
    .toBe(true);
  await emitTauriEvent(page, "state-changed", {
    state: "recording_meeting",
    data: { profile: PROFILE, session_id: 1, meeting_id: "meeting-e2e-1" },
  });

  const stopButton = page.getByRole("button", { name: "Stop", exact: true });
  await expect(page.getByText("Live meeting")).toBeVisible();
  await expect(stopButton).toBeVisible();

  // Exercise the streamed-segment path through the same Channel mechanism a
  // real recording session uses.
  await sendChannelMessage(page, "start_meeting_recording", {
    text: "hello from the stub",
    start_time: 0,
    end_time: 1,
    is_final: true,
    language: "en",
    confidence: 0.9,
    speaker: null,
  });
  await expect(page.getByText("hello from the stub")).toBeVisible();

  await stopButton.click();
  await expect
    .poll(async () => (await stubbedCalls(page)).some((call) => call.cmd === "stop_meeting_recording"))
    .toBe(true);
  await emitTauriEvent(page, "state-changed", { state: "ready", data: { profile: PROFILE } });

  // Back to a non-recording state: the live session card (and its Stop
  // button) is gone. The optimistic post-stop load takes the user to the
  // meeting detail view rather than the idle action buttons, so assert on
  // the absence of the recording UI rather than presence of "Meeting".
  await expect(stopButton).not.toBeVisible();
  await expect(page.getByText("Live meeting")).not.toBeVisible();
});
