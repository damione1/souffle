import type { TranscriptionRuntimePhase } from "../../types";

export type TranscriptionModelOperationState = "idle" | "downloading" | "loading";

export function runtimePhaseHeroLabel(phase: TranscriptionRuntimePhase): string {
  switch (phase) {
    case "download_required":
      return "Download required";
    case "load_required":
      return "Load required";
    case "ready":
      return "Model ready";
  }
  return phase satisfies never;
}

export function runtimePhaseAvailabilityLabel(phase: TranscriptionRuntimePhase): string {
  switch (phase) {
    case "download_required":
      return "Not downloaded";
    case "load_required":
      return "Downloaded";
    case "ready":
      return "Ready";
  }
  return phase satisfies never;
}

export function runtimePhasePillClass(phase: TranscriptionRuntimePhase): string {
  switch (phase) {
    case "download_required":
      return "pill-muted";
    case "load_required":
      return "pill-warning";
    case "ready":
      return "pill-success";
  }
  return phase satisfies never;
}
