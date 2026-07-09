/**
 * Stubs the Tauri IPC layer (`window.__TAURI_INTERNALS__`) for browser-only
 * Playwright smoke tests. `tauri-driver` (the WebDriver bridge that would
 * drive a real Tauri window) does not support macOS, so these tests run the
 * built Svelte app in a plain Chromium tab via the Vite dev server and fake
 * the backend at the IPC boundary instead.
 *
 * How it works: the generated command bindings (`src/lib/types/generated.ts`)
 * all funnel through `invoke()` from `@tauri-apps/api/core`, which reads
 * `window.__TAURI_INTERNALS__.invoke`. This installs a minimal stand-in for
 * that object — modeled on Tauri's own `@tauri-apps/api/mocks` helper
 * (`mockIPC` / `mockWindows`) — that:
 *   - resolves `invoke(cmd, args)` from a data-driven `cmd -> response` map
 *     (see `DEFAULT_RESPONSES`), so tests can override per command without
 *     touching the shim itself;
 *   - auto-handles the `plugin:event|listen` / `unlisten` / `emit` calls
 *     `@tauri-apps/api/event`'s `listen()` makes, so `emitTauriEvent` below
 *     can push a synthetic backend event (e.g. `state-changed`) to whatever
 *     the app already subscribed to;
 *   - captures any `Channel` passed as a command argument (e.g.
 *     `start_meeting_recording`'s segment channel) so `sendChannelMessage`
 *     can push streamed messages through it, the same way a real streaming
 *     command would.
 *
 * The injected function runs inside the page, so it cannot close over
 * anything from this module — only over the JSON-serializable `responses`
 * argument Playwright passes through `addInitScript`.
 */
import type { Page } from "@playwright/test";
import {
  mockCatalog,
  mockDictationEntry,
  mockMeeting,
  mockMeetingList,
  mockRuntimeStatus,
  mockSettings,
  mockShortcuts,
  mockSummaryProvidersStatus,
} from "../../src/lib/test-helpers/fixtures";

/** Response map keyed by the snake_case Tauri command name (as emitted by
 * tauri-specta), matching what `souffle_lib::lib::specta_builder` registers. */
export const DEFAULT_RESPONSES: Record<string, unknown> = {
  get_machine_state: { state: "ready", data: { profile: mockRuntimeStatus.profile } },
  get_settings: { ...mockSettings, audio_device: null, last_seen_version: "" },
  save_settings: null,
  get_transcription_catalog: mockCatalog,
  get_model_status: mockRuntimeStatus,
  get_app_version: "0.1.0",
  get_shortcuts: mockShortcuts,
  check_summary_providers: mockSummaryProvidersStatus,
  list_todays_calendar_events: [],
  list_calendars: [],
  list_meetings: mockMeetingList,
  get_meeting: mockMeeting,
  list_dictation_entries: [mockDictationEntry],
  list_dictionary: [],
  get_data_stats: { meetings_count: 0, dictation_entries_count: 0, database_bytes: 0 },
  recover_state: { state: "ready", data: { profile: mockRuntimeStatus.profile } },
  // Recording lifecycle — overridden per test where the flow matters.
  start_meeting_recording: null,
  resume_meeting_recording: null,
  stop_meeting_recording: "meeting-e2e-1",
  start_transcription: null,
  stop_transcription: null,
  add_dictation_entry: null,
  take_sleep_paused_meeting: null,
};

export interface TauriStubOptions {
  /** Overrides/additions merged over `DEFAULT_RESPONSES`, keyed by command name. */
  responses?: Record<string, unknown>;
}

/** Install the IPC stub. Must be called before `page.goto(...)` (it uses
 * `addInitScript`, which only affects documents navigated to afterwards). */
export async function installTauriStub(page: Page, options: TauriStubOptions = {}): Promise<void> {
  const responses = { ...DEFAULT_RESPONSES, ...options.responses };

  // Skip the first-run "grant permissions" walkthrough (App.svelte gates it
  // on this key) — it's a modal overlay unrelated to the flows under test
  // and would otherwise block every click.
  await page.addInitScript(() => {
    try {
      window.localStorage.setItem("permissionsOnboarded", "1");
    } catch {
      // Storage may be unavailable in some contexts; the dialog just shows.
    }
  });

  await page.addInitScript((responses: Record<string, unknown>) => {
    interface StubState {
      calls: Array<{ cmd: string; args: unknown }>;
      channels: Record<string, { id: number }>;
      runCallback: (id: number, data: unknown) => void;
    }

    const listeners = new Map<string, number[]>();
    const callbacks = new Map<number, (data: unknown) => unknown>();

    function registerCallback(cb: (data: unknown) => unknown, once = false): number {
      const id = Math.floor(Math.random() * 1_000_000_000);
      callbacks.set(id, (data: unknown) => {
        if (once) callbacks.delete(id);
        return cb && cb(data);
      });
      return id;
    }

    function unregisterCallback(id: number) {
      callbacks.delete(id);
    }

    function runCallback(id: number, data: unknown) {
      const cb = callbacks.get(id);
      if (cb) {
        cb(data);
      } else {
        console.warn(`[tauri-stub] no callback registered for id ${id}`);
      }
    }

    function isEventPluginInvoke(cmd: string): boolean {
      return cmd.startsWith("plugin:event|");
    }

    function handleEventPlugin(cmd: string, args: any): unknown {
      switch (cmd) {
        case "plugin:event|listen": {
          if (!listeners.has(args.event)) listeners.set(args.event, []);
          listeners.get(args.event)!.push(args.handler);
          return args.handler;
        }
        case "plugin:event|emit": {
          const handlers = listeners.get(args.event) ?? [];
          for (const handler of handlers) runCallback(handler, args);
          return null;
        }
        case "plugin:event|unlisten": {
          const handlers = listeners.get(args.event);
          if (handlers) {
            const index = handlers.indexOf(args.eventId);
            if (index !== -1) handlers.splice(index, 1);
          }
          return null;
        }
        default:
          return null;
      }
    }

    const state: StubState = { calls: [], channels: {}, runCallback };
    (window as any).__soufflStub = state;

    async function invoke(cmd: string, args?: any) {
      state.calls.push({ cmd, args });

      if (isEventPluginInvoke(cmd)) {
        return handleEventPlugin(cmd, args);
      }

      // A `Channel` argument serializes itself with a `toJSON()` that
      // returns `__CHANNEL__:<id>`, but the mock invoke path never
      // serializes — `args.channel` is still the live Channel instance, so
      // its `.id` can be driven directly via `runCallback`.
      if (args && typeof args === "object") {
        for (const [key, value] of Object.entries(args)) {
          if (value && typeof value === "object" && typeof (value as any).id === "number" && typeof (value as any).onmessage !== "undefined") {
            state.channels[cmd] = value as { id: number };
          } else if (key === "channel" && value && typeof value === "object") {
            state.channels[cmd] = value as { id: number };
          }
        }
      }

      if (Object.prototype.hasOwnProperty.call(responses, cmd)) {
        const value = responses[cmd];
        if (value instanceof Error) throw value;
        // `get_meeting` must echo the requested id: the frontend compares
        // the loaded meeting's `id` against `app.currentMeetingId` in a
        // reactive effect and keeps re-fetching until they match, so a
        // fixed fixture id here would spin forever instead of settling.
        if (cmd === "get_meeting" && value && typeof value === "object" && args?.id) {
          return { ...(value as object), id: args.id };
        }
        return value;
      }

      console.warn(`[tauri-stub] no response stubbed for command "${cmd}", returning null`);
      return null;
    }

    (window as any).__TAURI_INTERNALS__ = (window as any).__TAURI_INTERNALS__ ?? {};
    (window as any).__TAURI_INTERNALS__.invoke = invoke;
    (window as any).__TAURI_INTERNALS__.transformCallback = registerCallback;
    (window as any).__TAURI_INTERNALS__.unregisterCallback = unregisterCallback;
    (window as any).__TAURI_INTERNALS__.runCallback = runCallback;
    (window as any).__TAURI_INTERNALS__.metadata = {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    };
  }, responses);
}

/** Simulate a backend-pushed event (`StateChanged`, `MeetingFinalized`, ...)
 * reaching whatever the app already registered via `events.x.listen(...)`. */
export async function emitTauriEvent(page: Page, event: string, payload: unknown): Promise<void> {
  await page.evaluate(
    ([event, payload]) => (window as any).__TAURI_INTERNALS__.invoke("plugin:event|emit", { event, payload }),
    [event, payload] as const,
  );
}

/** Push a streamed message through the `Channel` captured for `cmd` (the
 * command name whose args included a `Channel`, e.g. `start_meeting_recording`). */
export async function sendChannelMessage(page: Page, cmd: string, message: unknown, index = 0): Promise<void> {
  await page.evaluate(
    ([cmd, message, index]) => {
      const stub = (window as any).__soufflStub;
      const channel = stub?.channels?.[cmd as string];
      if (!channel) {
        throw new Error(`[tauri-stub] no channel captured for command "${cmd}"`);
      }
      stub.runCallback(channel.id, { message, index });
    },
    [cmd, message, index] as const,
  );
}

/** All `{cmd, args}` pairs invoked so far, in order — for assertions like
 * "the app called start_meeting_recording exactly once". */
export async function stubbedCalls(page: Page): Promise<Array<{ cmd: string; args: unknown }>> {
  return page.evaluate(() => (window as any).__soufflStub?.calls ?? []);
}
