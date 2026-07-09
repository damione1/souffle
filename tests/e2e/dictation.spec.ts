import { expect, test } from "@playwright/test";
import { mockRuntimeStatus } from "../../src/lib/test-helpers/fixtures";
import { emitTauriEvent, installTauriStub, stubbedCalls } from "./tauri-stub";

const PROFILE = mockRuntimeStatus.profile;

test("toggling dictation from the UI returns to idle after stopping", async ({ page }) => {
  await installTauriStub(page);
  await page.goto("/");

  const dictateButton = page.getByRole("button", { name: "Dictate" });
  await expect(dictateButton).toBeVisible();
  await dictateButton.click();

  await expect
    .poll(async () => (await stubbedCalls(page)).some((call) => call.cmd === "start_transcription"))
    .toBe(true);
  await emitTauriEvent(page, "state-changed", {
    state: "recording_dictation",
    data: { profile: PROFILE, session_id: 1 },
  });

  const stopButton = page.getByRole("button", { name: "Stop", exact: true });
  await expect(page.getByText("Dictating")).toBeVisible();
  await expect(stopButton).toBeVisible();

  await stopButton.click();
  await expect
    .poll(async () => (await stubbedCalls(page)).some((call) => call.cmd === "stop_transcription"))
    .toBe(true);
  await emitTauriEvent(page, "state-changed", { state: "ready", data: { profile: PROFILE } });

  // Dictation has no detail view to fall into, so this goes all the way
  // back to the idle home screen with both start buttons showing again.
  await expect(stopButton).not.toBeVisible();
  await expect(page.getByRole("button", { name: "Dictate" })).toBeVisible();
  await expect(page.getByRole("button", { name: /^Meeting\b/ })).toBeVisible();
});
