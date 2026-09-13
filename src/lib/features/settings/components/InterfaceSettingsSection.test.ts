import { describe, expect, it, vi, afterEach } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/svelte";
import InterfaceSettingsSection from "./InterfaceSettingsSection.svelte";

const openPermissionsRepair = vi.fn();

vi.mock("../open", () => ({
  openPermissionsRepair: () => openPermissionsRepair(),
  openSettings: vi.fn(),
}));

const baseProps = {
  theme: "dark" as const,
  locale: "en",
  autoPaste: false,
  pasteDelayMs: 100,
  pasteMethod: "clipboard" as const,
  toggleShortcut: "CommandOrControl+Shift+Space",
  pttShortcut: "",
  recordingField: null as "toggle" | "ptt" | null,
  shortcutError: "",
  modifierTapStatus: null as { installed: boolean } | null,
  onThemeChange: vi.fn(),
  onLocaleChange: vi.fn(),
  onAutoPasteChange: vi.fn(),
  onPasteDelayChange: vi.fn(),
  onPasteMethodChange: vi.fn(),
  onStartRecording: vi.fn(),
  onClearShortcut: vi.fn(),
  formatShortcut: (s: string) => s || "Not set",
  pillHidden: false,
  onPillHiddenChange: vi.fn(),
  // What `getNativeShortcuts` hands the store at startup.
  nativeShortcuts: ["Fn", "MetaRight", "F5", "F8"],
};

describe("InterfaceSettingsSection native tap banner (SOU-116)", () => {
  afterEach(() => {
    cleanup();
    openPermissionsRepair.mockClear();
  });

  it("shows the banner when a native shortcut is bound and the tap is missing (AC2)", () => {
    render(InterfaceSettingsSection, {
      props: {
        ...baseProps,
        pttShortcut: "Fn",
        modifierTapStatus: { installed: false },
      },
    });

    expect(
      screen.getByText(
        /single-key shortcut won't work until Accessibility is granted/i,
      ),
    ).toBeTruthy();
  });

  it("shows the banner when Toggle is native and the tap is missing (SOU-115)", () => {
    render(InterfaceSettingsSection, {
      props: {
        ...baseProps,
        toggleShortcut: "Fn",
        pttShortcut: "",
        modifierTapStatus: { installed: false },
      },
    });

    expect(
      screen.getByText(
        /single-key shortcut won't work until Accessibility is granted/i,
      ),
    ).toBeTruthy();
  });

  it("hides the banner when no native shortcut is bound (AC6)", () => {
    render(InterfaceSettingsSection, {
      props: {
        ...baseProps,
        pttShortcut: "",
        modifierTapStatus: { installed: false },
      },
    });

    expect(
      screen.queryByText(
        /single-key shortcut won't work until Accessibility is granted/i,
      ),
    ).toBeNull();
  });

  it("opens the permissions repair panel from the banner button (AC3)", async () => {
    render(InterfaceSettingsSection, {
      props: {
        ...baseProps,
        pttShortcut: "F5",
        modifierTapStatus: { installed: false },
      },
    });

    await fireEvent.click(
      screen.getByRole("button", { name: /review permissions/i }),
    );
    expect(openPermissionsRepair).toHaveBeenCalledOnce();
  });
});
