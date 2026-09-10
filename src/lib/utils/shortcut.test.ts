import { describe, expect, it } from "vitest";
import {
  isNativePttShortcut,
  keyEventToShortcut,
  modifierToShortcut,
  shortcutMissingModifier,
  shouldShowNativeTapBanner,
} from "./shortcut";

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", init);
}

describe("keyEventToShortcut", () => {
  it("ignores modifier-only keydowns", () => {
    expect(keyEventToShortcut(key({ key: "Meta", metaKey: true }))).toBeNull();
    expect(keyEventToShortcut(key({ key: "Shift", shiftKey: true }))).toBeNull();
  });

  it("maps cmd-shift-space to the stored accelerator", () => {
    expect(
      keyEventToShortcut(key({ key: " ", code: "Space", metaKey: true, shiftKey: true })),
    ).toBe("CommandOrControl+Shift+Space");
  });

  it("treats ctrl like cmd on the stored form", () => {
    expect(
      keyEventToShortcut(key({ key: "s", code: "KeyS", ctrlKey: true })),
    ).toBe("CommandOrControl+S");
  });

  it("maps function keys without a modifier", () => {
    expect(keyEventToShortcut(key({ key: "F6", code: "F6" }))).toBe("F6");
  });
});

describe("modifierToShortcut", () => {
  it("maps left/right modifiers via code", () => {
    expect(modifierToShortcut(key({ key: "Meta", code: "MetaLeft" }))).toBe("MetaLeft");
    expect(modifierToShortcut(key({ key: "Alt", code: "AltRight" }))).toBe("AltRight");
  });

  it("maps fn / Globe, including the Clear key some layouts report", () => {
    expect(modifierToShortcut(key({ key: "Fn", code: "Fn" }))).toBe("Fn");
    expect(modifierToShortcut(key({ key: "Clear", code: "NumLock" }))).toBe("Fn");
  });

  it("ignores ordinary keys", () => {
    expect(modifierToShortcut(key({ key: "F5", code: "F5" }))).toBeNull();
    expect(modifierToShortcut(key({ key: "a", code: "KeyA" }))).toBeNull();
  });
});

describe("shortcutMissingModifier", () => {
  it("rejects a bare letter", () => {
    expect(shortcutMissingModifier(key({ key: "a", code: "KeyA" }))).toBe(true);
  });

  it("accepts F-keys and modified keys", () => {
    expect(shortcutMissingModifier(key({ key: "F8", code: "F8" }))).toBe(false);
    expect(shortcutMissingModifier(key({ key: "a", code: "KeyA", metaKey: true }))).toBe(false);
  });
});

describe("shouldShowNativeTapBanner (SOU-116)", () => {
  it("shows only when a native PTT shortcut is bound and the tap is missing", () => {
    expect(shouldShowNativeTapBanner("Fn", { installed: false })).toBe(true);
    expect(shouldShowNativeTapBanner("F5", { installed: false })).toBe(true);
  });

  it("hides when no native shortcut is bound (AC6)", () => {
    expect(shouldShowNativeTapBanner("", { installed: false })).toBe(false);
    expect(shouldShowNativeTapBanner("CommandOrControl+Shift+Space", { installed: false })).toBe(
      false,
    );
  });

  it("hides while status is unknown or the tap is installed", () => {
    expect(shouldShowNativeTapBanner("Fn", null)).toBe(false);
    expect(shouldShowNativeTapBanner("Fn", { installed: true })).toBe(false);
  });

  it("mirrors the backend native PTT set", () => {
    expect(isNativePttShortcut("MetaRight")).toBe(true);
    expect(isNativePttShortcut("F4")).toBe(false);
  });
});
