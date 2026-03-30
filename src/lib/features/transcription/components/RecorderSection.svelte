<script lang="ts">
  import { ArrowDownToLine, Cpu, Mic } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import type { TranscriptionRuntimePhase } from "../../../types";
  import type { TranscriptionModelOperationState } from "../state";

  type RecorderPhase =
    | "locked_by_meeting"
    | "starting"
    | "recording"
    | "downloading"
    | "needs_download"
    | "loading"
    | "needs_load"
    | "ready";

  let {
    isStartingRecording,
    isRecording,
    lockedByMeeting,
    runtimePhase,
    modelOperationState,
    downloadFile,
    downloadedBytes,
    downloadTotalBytes,
    inputDevice,
    autoPaste,
    onDownloadModel,
    onLoadModel,
    onToggleRecording,
  }: {
    isStartingRecording: boolean;
    isRecording: boolean;
    lockedByMeeting: boolean;
    runtimePhase: TranscriptionRuntimePhase;
    modelOperationState: TranscriptionModelOperationState;
    downloadFile: string;
    downloadedBytes: number;
    downloadTotalBytes: number | null;
    inputDevice: string;
    autoPaste: boolean;
    onDownloadModel: () => void | Promise<void>;
    onLoadModel: () => void | Promise<void>;
    onToggleRecording: () => void | Promise<void>;
  } = $props();

  let phase = $derived.by((): RecorderPhase => {
    if (lockedByMeeting) return "locked_by_meeting";
    if (isStartingRecording) return "starting";
    if (isRecording) return "recording";
    if (modelOperationState === "downloading") return "downloading";
    if (runtimePhase === "download_required") return "needs_download";
    if (modelOperationState === "loading") return "loading";
    if (runtimePhase !== "ready") return "needs_load";
    return "ready";
  });

  const headingKeys: Record<RecorderPhase, string> = {
    locked_by_meeting: "recorder.heading_locked",
    starting: "recorder.heading_starting",
    recording: "recorder.heading_recording",
    downloading: "recorder.heading_default",
    needs_download: "recorder.heading_default",
    loading: "recorder.heading_default",
    needs_load: "recorder.heading_default",
    ready: "recorder.heading_default",
  };

  const descriptionKeys: Record<RecorderPhase, string> = {
    locked_by_meeting: "recorder.desc_locked",
    starting: "recorder.desc_starting",
    recording: "recorder.desc_recording",
    downloading: "recorder.desc_downloading",
    needs_download: "recorder.desc_needs_download",
    loading: "recorder.desc_loading",
    needs_load: "recorder.desc_needs_load",
    ready: "recorder.desc_ready",
  };

  const actionKeys: Record<RecorderPhase, string> = {
    locked_by_meeting: "recorder.action_locked",
    starting: "recorder.action_starting",
    recording: "recorder.action_recording",
    downloading: "recorder.action_downloading",
    needs_download: "recorder.action_needs_download",
    loading: "recorder.action_loading",
    needs_load: "recorder.action_needs_load",
    ready: "recorder.action_ready",
  };
</script>

<section class="surface-card flex flex-col items-center gap-4 text-center">
  <h3>{$t(headingKeys[phase])}</h3>
  <p class="text-text-secondary text-sm">{$t(descriptionKeys[phase])}</p>

  {#if phase === "downloading"}
    <button disabled aria-label={$t("recorder.downloading_model_aria")} class="record-button is-starting">
      <ArrowDownToLine size={40} aria-hidden="true" />
    </button>
  {:else if phase === "needs_download"}
    <button onclick={onDownloadModel} aria-label={$t("recorder.download_model_aria")} class="record-button">
      <ArrowDownToLine size={40} aria-hidden="true" />
    </button>
  {:else if phase === "loading" || phase === "needs_load"}
    <button
      onclick={onLoadModel}
      disabled={phase === "loading"}
      aria-label={$t("recorder.load_model_aria")}
      class="record-button"
      class:is-starting={phase === "loading"}
    >
      <Cpu size={40} aria-hidden="true" />
    </button>
  {:else}
    <button
      onclick={onToggleRecording}
      disabled={phase === "starting" || phase === "locked_by_meeting"}
      aria-label={phase === "recording" ? $t("recorder.stop_recording_aria") : $t("recorder.start_recording_aria")}
      class="record-button"
      class:is-starting={phase === "starting"}
      class:is-recording={phase === "recording"}
    >
      <Mic size={40} aria-hidden="true" />
    </button>
  {/if}

  <span class="text-sm text-text-secondary">{$t(actionKeys[phase])}</span>

  {#if phase === "downloading"}
    <div class="w-full max-w-xs">
      <ProgressBar
        value={downloadedBytes}
        max={downloadTotalBytes && downloadTotalBytes > 0 ? downloadTotalBytes : 100}
        label={downloadFile || $t("model_gate.preparing_download")}
      />
    </div>
  {/if}

  <div class="flex gap-6 mt-2">
    <div class="flex flex-col gap-0.5 text-center">
      <span class="field-label">{$t("recorder.input_label")}</span>
      <span class="text-sm">{inputDevice || $t("recorder.default_device")}</span>
    </div>
    <div class="flex flex-col gap-0.5 text-center">
      <span class="field-label">{$t("recorder.output_label")}</span>
      <span class="text-sm">{autoPaste ? $t("recorder.auto_paste") : $t("recorder.manual_copy")}</span>
    </div>
  </div>
</section>
