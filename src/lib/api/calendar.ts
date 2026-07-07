import { commands, unwrap } from "./generated";
import type { CalendarInfo, TodayCalendar } from "../types";

export async function listCalendars(): Promise<CalendarInfo[]> {
  return unwrap(commands.listCalendars());
}

export async function listTodaysCalendarEvents(): Promise<TodayCalendar> {
  return unwrap(commands.listTodaysCalendarEvents());
}
