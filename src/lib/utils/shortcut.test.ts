import { describe, expect, it } from "vitest";
import { keyEventToShortcut, shortcutMissingModifier } from "./shortcut";

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

describe("shortcutMissingModifier", () => {
  it("rejects a bare letter", () => {
    expect(shortcutMissingModifier(key({ key: "a", code: "KeyA" }))).toBe(true);
  });

  it("accepts F-keys and modified keys", () => {
    expect(shortcutMissingModifier(key({ key: "F8", code: "F8" }))).toBe(false);
    expect(shortcutMissingModifier(key({ key: "a", code: "KeyA", metaKey: true }))).toBe(false);
  });
});
