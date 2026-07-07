import { listTodaysCalendarEvents } from "../../api/calendar";
import { getAppState } from "../../stores/app.svelte";
import type { CalendarEvent, PermState } from "../../types";
import { errorMessage } from "../../utils";
import { createMeetingController } from "../meeting/controller.svelte";

function createCalendarControllerInstance() {
  const app = getAppState();

  let events = $state<CalendarEvent[]>([]);
  let permission = $state<PermState>("unknown");
  let statusMessage = $state("");

  /** Refresh today's events. A no-op while the integration is disabled so
   * callers can invoke it unconditionally. */
  async function refresh() {
    if (!app.settings.calendar_integration_enabled) {
      events = [];
      return;
    }
    try {
      const today = await listTodaysCalendarEvents();
      permission = today.permission;
      events = today.events;
      statusMessage = "";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /** One-click start: the event title becomes the meeting title and the
   * attendees ride along as structured context. */
  async function startFromEvent(event: CalendarEvent) {
    const meeting = createMeetingController();
    await meeting.startRecording({
      title: event.title,
      calendar: { event_id: event.id, participants: event.participants },
    });
  }

  return {
    get events() { return events; },
    get permission() { return permission; },
    get statusMessage() { return statusMessage; },
    get enabled() { return app.settings.calendar_integration_enabled; },
    refresh,
    startFromEvent,
  };
}

// Singleton, matching the other feature controllers.
let instance: ReturnType<typeof createCalendarControllerInstance> | null = null;

export function createCalendarController() {
  if (!instance) {
    instance = createCalendarControllerInstance();
  }
  return instance;
}

/** Reset the singleton for testing. */
export function resetCalendarControllerForTest() {
  instance = null;
}
