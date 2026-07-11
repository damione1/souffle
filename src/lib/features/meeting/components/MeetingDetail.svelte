<script lang="ts">
  import { t } from "svelte-i18n";
  import type { createMeetingController } from "../controller.svelte";
  import MeetingAudioPlayerSection from "./MeetingAudioPlayerSection.svelte";
  import MeetingHeaderSection from "./MeetingHeaderSection.svelte";
  import MeetingNotesSection from "./MeetingNotesSection.svelte";
  import MeetingSummarySection from "./MeetingSummarySection.svelte";
  import MeetingStructuredSummarySection from "./MeetingStructuredSummarySection.svelte";
  import MeetingTranscriptSection from "./MeetingTranscriptSection.svelte";
  import ConfirmAction from "../../../components/ui/ConfirmAction.svelte";
  import Spinner from "../../../components/ui/Spinner.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";

  let { controller }: { controller: ReturnType<typeof createMeetingController> } = $props();

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

</script>

<div class="flex flex-col gap-[18px]">
  {#if controller.statusMessage}
    <StatusBanner message={controller.statusMessage} variant="warning" />
  {/if}

  {#if controller.isLoadingMeeting || !controller.meeting}
    <div class="flex flex-col items-center gap-2 p-8 text-text-muted">
      <Spinner />
      <p class="text-sm">{$t("meeting_view.loading")}</p>
    </div>
  {:else}
    <MeetingHeaderSection
      meeting={controller.meeting}
      isRecordingMeeting={controller.isRecordingMeeting}
      isStopping={controller.isStopping}
      systemAudioStatus={controller.app.systemAudioStatus}
      {lockedByDictation}
      segmentCount={displaySegments.length}
      sessionCount={sessionCount}
      canResumeRecording={controller.canResumeRecording}
      isExporting={controller.isExporting}
      onBack={() => controller.closeMeeting()}
      onRename={(title) => void controller.renameMeeting(title)}
      onResumeRecording={controller.resumeRecording}
      onStopRecording={controller.stopRecording}
      onExport={(format) => controller.exportMeeting(format)}
    />

    <!-- Notes are the focus: what the user wrote, front and center. -->
    <MeetingNotesSection
      large
      notes={controller.notesDraft}
      saveState={controller.notesSaveState}
      onNotesChange={controller.onNotesChange}
    />

    {#if controller.audioSessions.length > 0}
      <MeetingAudioPlayerSection
        audioSessions={controller.audioSessions}
        seekTarget={controller.seekTarget}
        seekRequestId={controller.seekRequestId}
      />
    {/if}

    <!-- Transcript is the hero: prominent card right under the notes. -->
    <MeetingTranscriptSection
      segments={displaySegments}
      recordingSessions={controller.meeting.recording_sessions}
      liveSessionStartIndex={liveSessionStartIndex}
      isRecordingMeeting={controller.isRecordingMeeting}
      hasEditedTranscript={controller.meeting.edited_transcript != null}
      isEditing={controller.isEditingTranscript}
      editedTranscriptDraft={controller.editedTranscriptDraft}
      onStartEdit={controller.startEditingTranscript}
      onCancelEdit={controller.cancelEditingTranscript}
      onSaveEdit={controller.saveTranscriptEdit}
      onSaveAndSummarize={controller.saveTranscriptAndSummarize}
      onResetEdited={controller.resetEditedTranscript}
      onEditDraftChange={(value) => { controller.editedTranscriptDraft = value; }}
      onParagraphClick={controller.audioSessions.length > 0 ? controller.requestAudioSeek : undefined}
    />

    <!-- AI summary, generated from notes + transcript. -->
    <MeetingSummarySection
      meeting={controller.meeting}
      isRecordingMeeting={controller.isRecordingMeeting}
      segments={displaySegments}
      summaryAvailable={controller.summaryAvailable}
      summaryModels={controller.summaryModels}
      selectedModel={controller.selectedModel}
      onSelectModel={(modelId) => {
        controller.selectedModel = modelId;
      }}
      summaryTemplates={controller.summaryTemplates}
      selectedTemplateId={controller.selectedTemplateId}
      onSelectTemplate={(templateId) => {
        controller.selectedTemplateId = templateId;
      }}
      onSummarize={controller.summarizeMeeting}
      isSummarizing={controller.isSummarizing}
      summaryStream={controller.summaryStream}
      summaryStage={controller.summaryStage}
      summaryStageCurrent={controller.summaryStageCurrent}
      summaryStageTotal={controller.summaryStageTotal}
    />

    <MeetingStructuredSummarySection
      meeting={controller.meeting}
      isRecordingMeeting={controller.isRecordingMeeting}
      isSummarizing={controller.isSummarizing}
    />

    {#if !controller.isRecordingMeeting && controller.meeting.id}
      <div class="flex items-center gap-2 pt-1">
        <ConfirmAction
          label={$t("meeting_view.delete_meeting")}
          confirmLabel={$t("meeting_view.delete_confirm_label")}
          confirmMessage={$t("meeting_view.delete_confirm_msg")}
          variant="danger"
          onConfirm={controller.deleteMeeting}
        />
      </div>
    {/if}
  {/if}
</div>
