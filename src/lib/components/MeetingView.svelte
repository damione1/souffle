<script lang="ts">
  import { onMount } from "svelte";
  import { createMeetingController } from "../features/meeting/controller.svelte";
  import MeetingHeaderSection from "../features/meeting/components/MeetingHeaderSection.svelte";
  import MeetingSummarySection from "../features/meeting/components/MeetingSummarySection.svelte";
  import MeetingTranscriptSection from "../features/meeting/components/MeetingTranscriptSection.svelte";
  import NewMeetingSection from "../features/meeting/components/NewMeetingSection.svelte";
  import ConfirmAction from "./ui/ConfirmAction.svelte";
  import Spinner from "./ui/Spinner.svelte";
  import StatusBanner from "./ui/StatusBanner.svelte";

  const controller = createMeetingController();

  let lockedByDictation = $derived(controller.app.recordingMode === "dictation");

  let sessionCount = $derived(
    controller.meeting
      ? controller.meeting.recording_sessions.length + (controller.isRecordingMeeting ? 1 : 0)
      : (controller.isRecordingMeeting ? 1 : 0),
  );
  let displaySegments = $derived(
    controller.isRecordingMeeting
      ? [...(controller.meeting?.segments ?? []), ...controller.liveMeetingSegments]
      : (controller.meeting?.segments ?? []),
  );
  let liveSessionStartIndex = $derived(
    controller.isRecordingMeeting && Boolean(controller.meeting?.id)
      ? (controller.meeting?.segments.length ?? null)
      : null,
  );

  onMount(() => {
    void controller.mount();
  });

  $effect(() => {
    void controller.onMeetingSelectionChange(controller.app.currentMeetingId);
  });
</script>

<div class="flex flex-col gap-4">
  {#if controller.statusMessage}
    <StatusBanner message={controller.statusMessage} variant="warning" />
  {/if}

  {#if controller.isLoadingMeeting}
    <div class="flex flex-col items-center gap-2 p-8 text-text-muted">
      <Spinner />
      <p class="text-sm">Loading meeting...</p>
    </div>
  {:else if controller.meeting}
    <MeetingHeaderSection
      meeting={controller.meeting}
      isRecordingMeeting={controller.isRecordingMeeting}
      {lockedByDictation}
      segmentCount={displaySegments.length}
      sessionCount={sessionCount}
      canResumeRecording={controller.canResumeRecording}
      onBack={() => {
        controller.app.currentMeetingId = null;
        controller.app.currentView = "meeting-history";
      }}
      onNewMeeting={() => controller.startNew()}
      onResumeRecording={controller.resumeRecording}
      onStopRecording={controller.stopRecording}
    />

    <div class="grid grid-cols-2 gap-4 max-[700px]:grid-cols-1">
      <MeetingTranscriptSection
        segments={displaySegments}
        recordingSessions={controller.meeting.recording_sessions}
        liveSessionStartIndex={liveSessionStartIndex}
        isRecordingMeeting={controller.isRecordingMeeting}
      />

      <MeetingSummarySection
        meeting={controller.meeting}
        isRecordingMeeting={controller.isRecordingMeeting}
        segments={displaySegments}
        ollamaAvailable={controller.ollamaAvailable}
        summaryModels={controller.summaryModels}
        selectedModel={controller.selectedModel}
        onSelectModel={(modelId) => {
          controller.selectedModel = modelId;
        }}
        onSummarize={controller.summarizeMeeting}
        isSummarizing={controller.isSummarizing}
        summaryStream={controller.summaryStream}
      />
    </div>

    {#if !controller.isRecordingMeeting && controller.meeting.id}
      <div class="flex items-center gap-2 pt-2 border-t border-ghost-border">
        <ConfirmAction
          label="Delete meeting"
          confirmLabel="Yes, delete"
          confirmMessage="Delete this meeting permanently?"
          variant="danger"
          onConfirm={controller.deleteMeeting}
        />
      </div>
    {/if}
  {:else}
    <NewMeetingSection
      meetingTitle={controller.meetingTitle}
      {lockedByDictation}
      onMeetingTitleChange={(value) => {
        controller.meetingTitle = value;
      }}
      onStartRecording={controller.startRecording}
    />
  {/if}
</div>
