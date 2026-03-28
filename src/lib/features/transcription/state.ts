import type { TranscriptionRuntimePhase } from "../../types";

export type TranscriptionModelOperationState = "idle" | "downloading" | "loading";

export function runtimePhaseHeroLabelKey(phase: TranscriptionRuntimePhase): string {
  switch (phase) {
    case "download_required":
      return "runtime_phase.download_required";
    case "load_required":
      return "runtime_phase.load_required";
    case "ready":
      return "runtime_phase.model_ready";
  }
  return phase satisfies never;
}

export function runtimePhaseAvailabilityLabelKey(phase: TranscriptionRuntimePhase): string {
  switch (phase) {
    case "download_required":
      return "runtime_phase.not_downloaded";
    case "load_required":
      return "runtime_phase.downloaded";
    case "ready":
      return "runtime_phase.ready";
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
