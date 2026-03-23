import { listen, type EventName, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Manages a group of Tauri event listeners with automatic cleanup.
 * Returns an object with `add()` to register listeners and `cleanup()` to remove all at once.
 */
export function useEventListeners() {
  const unlistenFns: UnlistenFn[] = [];

  return {
    /** Register a Tauri event listener; automatically tracked for cleanup */
    add<T>(event: EventName, handler: (event: { payload: T }) => void) {
      listen<T>(event, handler).then((fn) => unlistenFns.push(fn));
    },

    /** Remove all registered event listeners */
    cleanup() {
      unlistenFns.forEach((fn) => fn());
      unlistenFns.length = 0;
    },
  };
}
