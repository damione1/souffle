/** Map a keydown event to the accelerator string the backend stores
 * (`CommandOrControl+Shift+Space`). Returns null for modifier-only or
 * unmapped keys. */
export function modifierToShortcut(event: KeyboardEvent): string | null {
  if (event.key === "Fn" || event.code === "Fn" || event.key === "Clear") return "Fn";
  if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) {
    return event.code; // e.g. MetaLeft, AltRight
  }
  return null;
}

export function keyEventToShortcut(event: KeyboardEvent): string | null {
  if (event.key === "Fn" || ["Control", "Shift", "Alt", "Meta"].includes(event.key)) return null;
  const parts: string[] = [];
  if (event.metaKey || event.ctrlKey) parts.push("CommandOrControl");
  if (event.shiftKey) parts.push("Shift");
  if (event.altKey) parts.push("Alt");
  const key = mapKey(event.code, event.key);
  if (!key) return null;
  parts.push(key);
  return parts.join("+");
}

/** Bare letter/digit/symbol keys need a modifier (or to be an F-key). */
export function shortcutMissingModifier(event: KeyboardEvent): boolean {
  return (
    !event.metaKey
    && !event.ctrlKey
    && !event.shiftKey
    && !event.altKey
    && !/^F\d{1,2}$/.test(event.key)
  );
}

/** True when `shortcut` is one of the single-key bindings the CGEventTap
 * handles (and which therefore need Accessibility) rather than the
 * global-shortcut plugin. The list is not declared here: it comes from
 * `modifier_shortcut::NATIVE_SHORTCUTS` through `getNativeShortcuts`. */
export function isNativeShortcut(
  shortcut: string,
  nativeShortcuts: readonly string[],
): boolean {
  return shortcut !== "" && nativeShortcuts.includes(shortcut);
}

/** Settings banner when a native Toggle or PTT key is bound and the tap is
 * known missing. `null` status = not yet attempted (startup delay); do not
 * flash (SOU-116). Toggle is included once SOU-115 can bind native keys. */
export function shouldShowNativeTapBanner(
  pttShortcut: string,
  tapStatus: { installed: boolean } | null,
  toggleShortcut: string,
  nativeShortcuts: readonly string[],
): boolean {
  const nativeBound =
    isNativeShortcut(pttShortcut, nativeShortcuts)
    || isNativeShortcut(toggleShortcut, nativeShortcuts);
  return nativeBound && tapStatus?.installed === false;
}

function mapKey(code: string, key: string): string | null {
  if (/^F\d{1,2}$/.test(key)) return key;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  const keyMap: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Escape: "Escape",
    Backspace: "Backspace",
    Tab: "Tab",
    ArrowUp: "ArrowUp",
    ArrowDown: "ArrowDown",
    ArrowLeft: "ArrowLeft",
    ArrowRight: "ArrowRight",
    Delete: "Delete",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Backquote: "Backquote",
    Minus: "Minus",
    Equal: "Equal",
    BracketLeft: "BracketLeft",
    BracketRight: "BracketRight",
    Backslash: "Backslash",
    Semicolon: "Semicolon",
    Quote: "Quote",
    Comma: "Comma",
    Period: "Period",
    Slash: "Slash",
  };
  return keyMap[code] || null;
}
