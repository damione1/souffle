import { expect, test } from "@playwright/test";
import { installTauriStub } from "./tauri-stub";

test("app boots to the home view with no console errors", async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));

  await installTauriStub(page);
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Dictate" })).toBeVisible();
  await expect(page.getByRole("button", { name: /^Meeting\b/ })).toBeVisible();

  expect(consoleErrors, `unexpected console errors:\n${consoleErrors.join("\n")}`).toEqual([]);
});
