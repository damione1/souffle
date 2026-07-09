import { commands, unwrap } from "./generated";
import type { SummaryProvidersStatus } from "../types";

export async function getSummaryProvidersStatus(): Promise<SummaryProvidersStatus> {
  return unwrap(commands.checkSummaryProviders());
}
