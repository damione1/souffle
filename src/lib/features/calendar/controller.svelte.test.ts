import { describe, it, expect, vi, beforeEach } from "vitest";
import type { CalendarEvent, TodayCalendar } from "../../types";

const mockListTodaysCalendarEvents = vi.fn<() => Promise<TodayCalendar>>();
const mockStartRecording = vi.fn<(options?: unknown) => Promise<void>>();

vi.mock("../../api/calendar", () => ({
  listTodaysCalendarEvents: (...a: unknown[]) =>
    mockListTodaysCalendarEvents(...(a as [])),
  listCalendars: vi.fn(),
}));

vi.mock("../meeting/controller.svelte", () => ({
  createMeetingController: () => ({ startRecording: mockStartRecording }),
}));

const mockAppState = {
  settings: { calendar_integration_enabled: true },
};

vi.mock("../../stores/app.svelte", () => ({
  getAppState: () => mockAppState,
}));

const { createCalendarController, resetCalendarControllerForTest } = await import(
  "./controller.svelte"
);

function makeEvent(overrides: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    id: "evt-1",
    title: "Sprint Planning",
    start: "2026-07-06T10:00:00Z",
    end: "2026-07-06T11:00:00Z",
    calendar_id: "cal-1",
    calendar_title: "Work",
    participants: [
      { name: "Alice", email: "alice@corp.com", is_organizer: true, is_current_user: false },
    ],
    location: null,
    url: null,
    description: null,
    ...overrides,
  };
}

describe("calendar controller", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetCalendarControllerForTest();
    mockAppState.settings.calendar_integration_enabled = true;
  });

  it("refresh loads today's events when the integration is enabled", async () => {
    const event = makeEvent();
    mockListTodaysCalendarEvents.mockResolvedValue({
      permission: "granted",
      events: [event],
    });

    const ctrl = createCalendarController();
    await ctrl.refresh();

    expect(ctrl.events).toEqual([event]);
    expect(ctrl.permission).toBe("granted");
    expect(ctrl.statusMessage).toBe("");
  });

  it("refresh is a no-op while the integration is disabled", async () => {
    mockAppState.settings.calendar_integration_enabled = false;

    const ctrl = createCalendarController();
    await ctrl.refresh();

    expect(mockListTodaysCalendarEvents).not.toHaveBeenCalled();
    expect(ctrl.events).toEqual([]);
  });

  it("refresh surfaces the denied permission state without events", async () => {
    mockListTodaysCalendarEvents.mockResolvedValue({
      permission: "denied",
      events: [],
    });

    const ctrl = createCalendarController();
    await ctrl.refresh();

    expect(ctrl.permission).toBe("denied");
    expect(ctrl.events).toEqual([]);
  });

  it("startFromEvent delegates title and calendar context to the meeting controller", async () => {
    mockStartRecording.mockResolvedValue(undefined);
    const event = makeEvent();

    const ctrl = createCalendarController();
    await ctrl.startFromEvent(event);

    expect(mockStartRecording).toHaveBeenCalledWith({
      title: "Sprint Planning",
      calendar: { event_id: "evt-1", participants: event.participants, description: null },
    });
  });

  it("refresh keeps events and reports the error when the query fails", async () => {
    mockListTodaysCalendarEvents.mockRejectedValue(new Error("boom"));

    const ctrl = createCalendarController();
    await ctrl.refresh();

    expect(ctrl.statusMessage).not.toBe("");
  });
});
