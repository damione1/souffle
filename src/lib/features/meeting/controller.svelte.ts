import { save as showSaveDialog } from "@tauri-apps/plugin-dialog";
import { addDictionaryEntry, addSessionCorrection } from "../../api/dictionary";
import {
  deleteMeeting as removeMeeting,
  exportMeetingFilename,
  exportMeetingToFile,
  getMeeting,
  getMeetingAudio,
  renameMeeting as applyMeetingRename,
  resumeMeetingRecording,
  saveEditedTranscript,
  applyLiveParagraphEdit as submitLiveParagraphEdit,
  saveMeetingNotes,
  startMeetingRecording,
  stopMeetingRecording,
  summarizeMeeting as runMeetingSummary,
  takeSleepPausedMeeting,
} from "../../api/meetings";
import { getSummaryProvidersStatus } from "../../api/summary";
import { getTranscriptionCatalog } from "../../api/transcription";
import { getAppState } from "../../stores/app.svelte";
import type { ExportFormat, MeetingAudioSession, MeetingCalendarContext, MeetingIdle, MeetingTranscript, SummaryModelDescriptor, SummarizeProgress, TranscriptionCatalog, TranscriptionSegment } from "../../types";
import { errorMessage } from "../../utils";
import { toSelectedTranscriptionProfile } from "../transcription/catalog";
import { ensureModelLoaded } from "../transcription/runtime";
import { type AudioSeekTarget, resolveAudioSeekTarget } from "./audio-map";
import { createLiveTranscript } from "./live-transcript.svelte";

function defaultMeetingTitle(): string {
  return `Meeting ${new Date().toLocaleDateString()}`;
}

/** Extra silence tolerated after the banner first appears before auto-stop
 * kicks in, on top of the configured silence threshold. */
const SILENCE_AUTOSTOP_GRACE_SECONDS = 120;

function createMeetingControllerInstance() {
  const app = getAppState();

  let statusMessage = $state("");
  let ollamaAvailable = $state(false);
  let appleIntelligenceAvailable = $state(false);
  let summaryModels = $state<SummaryModelDescriptor[]>([]);
  let selectedModel = $state("");
  // Explicit template pick for this session; empty means "follow the default
  // template from settings", which the Generate dropdown shows preselected.
  let pickedTemplateId = $state("");
  let selectedTemplateId = $derived(
    pickedTemplateId
      && app.settings.summary_templates.some((template) => template.id === pickedTemplateId)
      ? pickedTemplateId
      : app.settings.default_summary_template_id,
  );
  let isSummarizing = $state(false);
  let summaryStream = $state("");
  // Live progress label for the current generation (map chunk N/M, combine
  // round, structured extraction); null once the run finishes or has not
  // reported a stage yet.
  let summaryStage = $state<SummarizeProgress["stage"] | null>(null);
  let summaryStageCurrent = $state<number | null>(null);
  let summaryStageTotal = $state<number | null>(null);
  let transcriptionCatalog = $state<TranscriptionCatalog | null>(null);

  let isEditingTranscript = $state(false);
  let editedTranscriptDraft = $state("");
  let isExporting = $state(false);

  const NOTES_DEBOUNCE_MS = 800;
  let notesDraft = $state("");
  let notesSaveState = $state<"idle" | "pending" | "saved">("idle");
  let notesTimer: ReturnType<typeof setTimeout> | null = null;

  // Meeting-idle ("meeting seems to be over") banner state.
  let idleSignal = $state<MeetingIdle | null>(null);
  // True once the user dismissed the current silence episode ("keep
  // recording"); suppresses further banners until a segment re-arms it.
  let idleDismissed = $state(false);

  let isRecordingMeeting = $derived(
    app.machineState.state === "recording_meeting"
    || (app.machineState.state === "stopping"
        && typeof app.machineState.data?.was_recording === "object"),
  );
  // Incremental grouper for the compact live view (LiveSessionCard): bounded
  // work per segment instead of re-grouping the whole meeting each time.
  const liveTranscript = createLiveTranscript(1.5);
  // Raw finalized segments, still needed by the meeting detail view's full
  // transcript section (buildMeetingTranscriptBlocks operates on segments,
  // not paragraphs). Mutated via push, not clone-per-segment.
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);

  let meeting = $state<MeetingTranscript | null>(null);
  let isLoadingMeeting = $state(false);
  // Recorded audio files for the open meeting (empty when recording was off,
  // or nothing survived retention) — drives whether the player bar shows.
  let audioSessions = $state<MeetingAudioSession[]>([]);
  // A paragraph-click seek request for the player: `requestId` changes on
  // every click (even to the same target) so an `$effect` watching it always
  // fires, including re-clicking the same paragraph after manually pausing.
  let seekTarget = $state<AudioSeekTarget | null>(null);
  let seekRequestId = $state(0);
  // True from the moment Stop is clicked until the backend finishes draining +
  // saving (machine leaves "stopping"). Drives the Stop button's spinner and
  // guards against double-stop. `stopRequested` covers the brief gap before the
  // machine reports "stopping".
  let stopRequested = $state(false);
  let isStopping = $derived(stopRequested || app.machineState.state === "stopping");
  // "load_required" stays resumable: resumeRecording reloads the model on
  // demand, same as a fresh start after an idle unload.
  let canResumeRecording = $derived(
    Boolean(meeting?.id)
    && !isRecordingMeeting
    && !isLoadingMeeting
    && !isSummarizing
    && (app.transcriptionRuntimePhase === "ready"
      || app.transcriptionRuntimePhase === "load_required"),
  );

  async function mount() {
    await Promise.all([refreshSummaryProviders(), loadTranscriptionCatalog()]);
  }

  async function loadTranscriptionCatalog() {
    try {
      transcriptionCatalog = await getTranscriptionCatalog();
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function onMeetingSelectionChange(id: string | null) {
    if (id && (!meeting || meeting.id !== id) && !isRecordingMeeting) {
      await loadMeeting(id);
    }
  }

  function syncSelectedModel(preferredModel?: string | null) {
    if (summaryModels.length === 0) {
      selectedModel = "";
      return;
    }
    if (preferredModel && summaryModels.some((model) => model.id === preferredModel)) {
      selectedModel = preferredModel;
      return;
    }
    if (app.settings.ollama_model && summaryModels.some((model) => model.id === app.settings.ollama_model)) {
      selectedModel = app.settings.ollama_model;
      return;
    }
    if (selectedModel && summaryModels.some((model) => model.id === selectedModel)) return;
    selectedModel = summaryModels[0].id;
  }

  async function loadMeeting(id: string) {
    isLoadingMeeting = true;
    try {
      meeting = await getMeeting(id);
      syncSelectedModel(meeting.summary_model);
      notesDraft = meeting.notes ?? "";
      notesSaveState = "idle";
      // Best-effort: a missing/unreadable recordings directory should never
      // block loading the meeting itself, the player bar just stays hidden.
      audioSessions = await getMeetingAudio(id).catch(() => []);
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isLoadingMeeting = false;
    }
  }

  /** A paragraph's timestamp was clicked: resolve it to a playable audio
   * target (no-op if the paragraph isn't attributed to a recorded session,
   * e.g. recording was off or the file didn't survive retention). */
  function requestAudioSeek(recordingSessionIndex: number | null, startTime: number) {
    const target = resolveAudioSeekTarget(recordingSessionIndex, startTime, audioSessions);
    if (!target) return;
    seekTarget = target;
    seekRequestId += 1;
  }

  /** The meeting id to write notes against: the accumulator id while
   * recording (the row doesn't exist in the DB yet), the row id after. */
  function notesTargetId(): string {
    const machineState = app.machineState;
    if (machineState.state === "recording_meeting") return machineState.data.meeting_id;
    return meeting?.id ?? "";
  }

  function onNotesChange(value: string) {
    notesDraft = value;
    notesSaveState = "pending";
    if (notesTimer) clearTimeout(notesTimer);
    notesTimer = setTimeout(() => void flushNotes(), NOTES_DEBOUNCE_MS);
  }

  async function flushNotes() {
    if (notesTimer) {
      clearTimeout(notesTimer);
      notesTimer = null;
    }
    if (notesSaveState !== "pending") return;
    const id = notesTargetId();
    if (!id) return;
    try {
      await saveMeetingNotes(id, notesDraft.trim() || null);
      notesSaveState = "saved";
    } catch (e) {
      statusMessage = errorMessage(e);
      notesSaveState = "idle";
    }
  }

  /** Speech activity (a new segment) or a fresh recording resets the idle
   * banner and re-arms it for the next episode of silence. */
  function clearIdleState() {
    idleSignal = null;
    idleDismissed = false;
  }

  async function refreshSummaryProviders() {
    try {
      const status = await getSummaryProvidersStatus();
      ollamaAvailable = status.ollama_available;
      appleIntelligenceAvailable = status.apple_intelligence_available;
      summaryModels = status.models.filter((model) => model.can_summarize);
      syncSelectedModel(meeting?.summary_model);
    } catch {
      ollamaAvailable = false;
      appleIntelligenceAvailable = false;
      summaryModels = [];
    }
  }

  async function startRecording(options?: {
    title?: string;
    calendar?: MeetingCalendarContext;
  }) {
    if (app.transcriptionRuntimePhase !== "ready") {
      // Model may have been unloaded by the idle timeout; reload through the
      // normal load flow before recording rather than failing on start.
      statusMessage = "";
      const ready = await ensureModelLoaded(app, transcriptionCatalog, (message) => { statusMessage = message; });
      if (!ready) {
        if (!statusMessage) statusMessage = "Load the model before starting a meeting.";
        return;
      }
    }
    try {
      const title = options?.title?.trim() || defaultMeetingTitle();
      const calendar = options?.calendar ?? null;
      liveTranscript.reset();
      liveMeetingSegments = [];
      statusMessage = "";
      summaryStream = "";
      meeting = null;
      notesDraft = "";
      notesSaveState = "idle";
      clearIdleState();
      const transcriptionProfile = toSelectedTranscriptionProfile(
        transcriptionCatalog,
        app.settings.transcription_engine_id,
        app.settings.transcription_model_id,
        app.settings.transcription_backend_id,
      );

      await startMeetingRecording(title, calendar, (segment) => {
        // Only skip empty finals; non-final (tentative) segments still flow
        // through so the live view can show them as a faded suffix.
        if (segment.is_final && !segment.text) return;
        const segmentIndex = liveMeetingSegments.length;
        liveTranscript.append(segment, segmentIndex);
        if (segment.is_final && segment.text) liveMeetingSegments.push(segment);
        clearIdleState();
      });

      meeting = {
        id: "",
        title,
        started_at: new Date().toISOString(),
        ended_at: null,
        duration_seconds: 0,
        transcription_profile: transcriptionProfile,
        recording_sessions: [],
        segments: [],
        summary: null,
        summary_is_stale: false,
        summary_model: null,
        summary_generated_at: null,
        structured_summary: null,
        edited_transcript: null,
        notes: null,
        calendar_event_id: calendar?.event_id ?? null,
        participants: calendar?.participants ?? [],
        speakers: [],
      };
    } catch (e) {
      statusMessage = errorMessage(e);
      liveTranscript.reset();
      liveMeetingSegments = [];
    }
  }

  async function resumeRecording() {
    if (!meeting || !meeting.id) return;

    try {
      const ready = await ensureModelLoaded(app, transcriptionCatalog, (message) => { statusMessage = message; });
      if (!ready) return;
      liveTranscript.reset();
      liveMeetingSegments = [];
      statusMessage = "";
      summaryStream = "";
      clearIdleState();

      await resumeMeetingRecording(meeting.id, (segment) => {
        if (segment.is_final && !segment.text) return;
        const segmentIndex = liveMeetingSegments.length;
        liveTranscript.append(segment, segmentIndex);
        if (segment.is_final && segment.text) liveMeetingSegments.push(segment);
        clearIdleState();
      });
    } catch (e) {
      statusMessage = errorMessage(e);
      liveTranscript.reset();
      liveMeetingSegments = [];
    }
  }

  async function stopRecording() {
    if (isStopping) return; // guard against double-stop
    stopRequested = true;
    clearIdleState();
    try {
      // Unsaved notes must reach the accumulator before it is persisted.
      await flushNotes();
      // Decoupled stop: returns the id immediately; the backend drains and
      // saves in the background and emits `meetingFinalized` when done.
      const id = await stopMeetingRecording();
      app.currentMeetingId = id;

      // Optimistically load the partially-persisted meeting so the detail view
      // has data the instant the machine flips to idle. The header + most
      // segments are already on disk from incremental persistence; the
      // `meetingFinalized` event reloads the authoritative version.
      try {
        meeting = await getMeeting(id);
        syncSelectedModel(meeting.summary_model);
        notesDraft = meeting.notes ?? "";
      } catch {
        // Header may not be queryable yet in a rare race; finalize reloads it.
      }
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      stopRequested = false;
    }
  }

  /** The backend finished draining + saving a stopped meeting. Reload the
   * now-complete record (ended_at, duration, all segments) and drop the live
   * buffer. No-op if the user already navigated elsewhere. */
  function handleMeetingFinalized(id: string) {
    if (app.currentMeetingId !== id && meeting?.id !== id) return;
    liveTranscript.reset();
    liveMeetingSegments = [];
    clearIdleState();
    void loadMeeting(id);
  }

  /** The backend aborted the recording session (machine went to Error).
   * The backend salvages the accumulated meeting to history before failing. */
  function handleRecordingAborted() {
    liveTranscript.reset();
    liveMeetingSegments = [];
    meeting = null;
    audioSessions = [];
    app.currentMeetingId = null;
    clearIdleState();
    statusMessage =
      "Recording was interrupted — the meeting recorded so far was saved to history.";
  }

  /** The backend detected the meeting has probably ended (silence or the
   * max-duration failsafe). Ignored outside an active meeting recording. */
  function handleMeetingIdle(payload: MeetingIdle) {
    if (!isRecordingMeeting) return;

    if (payload.reason === "max_duration") {
      if (isStopping) return;
      statusMessage = "Maximum meeting duration reached. Stopping the recording.";
      void stopRecording();
      return;
    }

    // Silence: suppressed once the user chose to keep recording, until
    // speech resumes and re-arms it.
    if (idleDismissed) return;
    idleSignal = payload;
    if (payload.idle_seconds >= payload.threshold_seconds + SILENCE_AUTOSTOP_GRACE_SECONDS) {
      void stopRecording();
    }
  }

  /** "Keep recording": suppress the banner until speech resumes. */
  function dismissIdle() {
    idleDismissed = true;
    idleSignal = null;
  }

  function recordingMeetingId(): string {
    const machineState = app.machineState;
    if (machineState.state === "recording_meeting") return machineState.data.meeting_id;
    return meeting?.id ?? "";
  }

  function redistributeSegmentTexts(segmentStart: number, segmentEnd: number, newText: string) {
    const words = newText.trim().split(/\s+/).filter(Boolean);
    const slice = liveMeetingSegments.slice(segmentStart, segmentEnd);
    if (slice.length === 0) return;
    if (slice.length === 1) {
      slice[0].text = newText.trim();
      return;
    }
    for (let i = 0; i < slice.length; i++) {
      if (i + 1 < slice.length) {
        slice[i].text = words[i] ?? "";
      } else {
        slice[i].text = words.slice(i).join(" ");
      }
    }
  }

  async function addDictionaryAlias(term: string, pronunciation: string | null) {
    const trimmedTerm = term.trim();
    if (!trimmedTerm) return;

    const trimmedPronunciation = pronunciation?.trim() || null;
    try {
      await addDictionaryEntry(trimmedTerm, trimmedPronunciation, null);
      if (
        isRecordingMeeting
        && trimmedPronunciation
        && trimmedPronunciation.toLowerCase() !== trimmedTerm.toLowerCase()
      ) {
        try {
          await addSessionCorrection(trimmedPronunciation, trimmedTerm);
        } catch {
          // Session hint is best-effort; the dictionary entry still persists.
        }
      }
    } catch (e) {
      statusMessage = errorMessage(e);
      throw e;
    }
  }

  async function applyLiveParagraphEdit(paragraphId: number, newText: string) {
    const trimmed = newText.trim();
    if (!trimmed || !isRecordingMeeting) return;

    const updated = liveTranscript.editParagraph(paragraphId, trimmed);
    if (!updated) return;

    const { start, end } = updated.segmentRange;
    if (end <= start || end > liveMeetingSegments.length) return;

    redistributeSegmentTexts(start, end, trimmed);

    const meetingId = recordingMeetingId();
    if (!meetingId) return;

    try {
      await submitLiveParagraphEdit(meetingId, start, end, trimmed);
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /** The system woke from sleep (or the webview visibility turned to
   * visible, as a belt-and-braces recheck in case the wake event fired while
   * the webview was suspended). Ask the backend whether a meeting was
   * stopped by sleep and, if so, reload it and auto-resume recording through
   * the normal resume flow. Idempotent: `take_sleep_paused_meeting` clears
   * its state on read, so a second call (event + visibilitychange both
   * firing) is a harmless no-op. */
  async function resumeAfterSystemWake() {
    let meetingId: string | null;
    try {
      meetingId = await takeSleepPausedMeeting();
    } catch (e) {
      statusMessage = errorMessage(e);
      return;
    }
    if (!meetingId || isRecordingMeeting) return;

    await loadMeeting(meetingId);
    if (!meeting || meeting.id !== meetingId) return; // load failed; loadMeeting already reported it

    // resumeRecording reports its own failure via statusMessage and leaves
    // the meeting loaded (canResumeRecording) so the user can retry by hand.
    await resumeRecording();
    if (!statusMessage) {
      statusMessage = "Recording resumed after sleep.";
    }
  }

  /** Leave the detail view: clear the open meeting and return to the list. */
  function closeMeeting() {
    void flushNotes();
    meeting = null;
    audioSessions = [];
    seekTarget = null;
    liveTranscript.reset();
    liveMeetingSegments = [];
    statusMessage = "";
    summaryStream = "";
    app.currentMeetingId = null;
  }

  /** Rename the open meeting (works while recording too). */
  async function renameMeeting(title: string) {
    if (!meeting) return;
    const trimmed = title.trim();
    if (!trimmed || trimmed === meeting.title) return;
    try {
      // A live meeting that hasn't been stopped yet has the accumulator id
      // in the machine state, not in the (placeholder) meeting object.
      const machineState = app.machineState;
      const id = meeting.id
        || (machineState.state === "recording_meeting" ? machineState.data.meeting_id : "");
      if (!id) return;
      await applyMeetingRename(id, trimmed);
      meeting = { ...meeting, title: trimmed };
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function summarizeMeeting() {
    if (!selectedModel || !meeting || !meeting.id) return;
    const meetingId = meeting.id;
    isSummarizing = true;
    summaryStream = "";
    summaryStage = null;
    summaryStageCurrent = null;
    summaryStageTotal = null;
    statusMessage = "";

    try {
      // Stream tokens for live preview only. Completion is driven by the
      // command resolving (it returns after the summary is saved), NOT by the
      // streaming `done` flag — a dropped final chunk must not leave the UI
      // stuck "Generating…". Text only carries real tokens during the
      // "final" stage; map/combine/extract markers arrive with empty text
      // and just update the stage label.
      await runMeetingSummary(meetingId, selectedModel, selectedTemplateId || null, (progress: SummarizeProgress) => {
        if (progress.text) summaryStream += progress.text;
        summaryStage = progress.stage;
        summaryStageCurrent = progress.current ?? null;
        summaryStageTotal = progress.total ?? null;
      });
      const loadedMeeting = await getMeeting(meetingId);
      meeting = loadedMeeting;
      syncSelectedModel(meeting.summary_model);
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isSummarizing = false;
      summaryStage = null;
      summaryStageCurrent = null;
      summaryStageTotal = null;
    }
  }

  function startEditingTranscript() {
    if (!meeting) return;
    // Use edited_transcript if available, otherwise join segments
    editedTranscriptDraft = meeting.edited_transcript
      ?? meeting.segments.map((s) => s.text).join(" ");
    isEditingTranscript = true;
  }

  function cancelEditingTranscript() {
    isEditingTranscript = false;
    editedTranscriptDraft = "";
  }

  async function saveTranscriptEdit() {
    if (!meeting || !meeting.id) return;
    try {
      const textToSave = editedTranscriptDraft.trim() || null;
      await saveEditedTranscript(meeting.id, textToSave);
      meeting = await getMeeting(meeting.id);
      isEditingTranscript = false;
      editedTranscriptDraft = "";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function saveTranscriptAndSummarize() {
    await saveTranscriptEdit();
    if (!isEditingTranscript) {
      await summarizeMeeting();
    }
  }

  async function resetEditedTranscript() {
    if (!meeting || !meeting.id) return;
    try {
      await saveEditedTranscript(meeting.id, null);
      meeting = await getMeeting(meeting.id);
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function deleteMeeting() {
    if (!meeting || !meeting.id) return;
    try {
      await removeMeeting(meeting.id);
      closeMeeting();
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /**
   * Export the current meeting to a file the user picks via the native save
   * dialog. The suggested filename (and its extension) come from the
   * backend (`export_meeting_filename`) so the slugify/date-formatting rules
   * live in exactly one place; the dialog plugin only needs the extension
   * for its filter, which we read back off that filename.
   */
  async function exportMeeting(format: ExportFormat) {
    if (!meeting || !meeting.id || isRecordingMeeting) return;
    isExporting = true;
    statusMessage = "";
    try {
      const filename = await exportMeetingFilename(meeting.id, format);
      const extension = filename.split(".").pop() ?? format;
      const path = await showSaveDialog({
        defaultPath: filename,
        filters: [{ name: extension.toUpperCase(), extensions: [extension] }],
      });
      if (!path) return; // user cancelled the dialog
      await exportMeetingToFile(meeting.id, format, path);
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isExporting = false;
    }
  }

  return {
    get app() { return app; },
    get statusMessage() { return statusMessage; },
    get ollamaAvailable() { return ollamaAvailable; },
    get appleIntelligenceAvailable() { return appleIntelligenceAvailable; },
    get summaryAvailable() { return summaryModels.length > 0; },
    get summaryModels() { return summaryModels; },
    get selectedModel() { return selectedModel; },
    set selectedModel(modelId: string) { selectedModel = modelId; },
    get summaryTemplates() { return app.settings.summary_templates; },
    get selectedTemplateId() { return selectedTemplateId; },
    set selectedTemplateId(templateId: string) { pickedTemplateId = templateId; },
    get isSummarizing() { return isSummarizing; },
    get summaryStream() { return summaryStream; },
    get summaryStage() { return summaryStage; },
    get summaryStageCurrent() { return summaryStageCurrent; },
    get summaryStageTotal() { return summaryStageTotal; },
    get isRecordingMeeting() { return isRecordingMeeting; },
    get liveTranscript() { return liveTranscript; },
    get liveMeetingSegments() { return liveMeetingSegments; },
    get meeting() { return meeting; },
    get audioSessions() { return audioSessions; },
    get seekTarget() { return seekTarget; },
    get seekRequestId() { return seekRequestId; },
    requestAudioSeek,
    get isLoadingMeeting() { return isLoadingMeeting; },
    get isStopping() { return isStopping; },
    get canResumeRecording() { return canResumeRecording; },
    get isEditingTranscript() { return isEditingTranscript; },
    get isExporting() { return isExporting; },
    get editedTranscriptDraft() { return editedTranscriptDraft; },
    set editedTranscriptDraft(value: string) { editedTranscriptDraft = value; },
    get notesDraft() { return notesDraft; },
    get notesSaveState() { return notesSaveState; },
    get idleSignal() { return idleSignal; },
    get idleDismissed() { return idleDismissed; },
    onNotesChange,
    flushNotes,
    mount,
    onMeetingSelectionChange,
    refreshSummaryProviders,
    startRecording,
    resumeRecording,
    stopRecording,
    closeMeeting,
    renameMeeting,
    summarizeMeeting,
    deleteMeeting,
    exportMeeting,
    startEditingTranscript,
    cancelEditingTranscript,
    saveTranscriptEdit,
    saveTranscriptAndSummarize,
    resetEditedTranscript,
    handleRecordingAborted,
    handleMeetingFinalized,
    handleMeetingIdle,
    dismissIdle,
    applyLiveParagraphEdit,
    addDictionaryAlias,
    resumeAfterSystemWake,
  };
}

/** Called from the global StateChanged listener when a meeting session
 * is aborted by the backend. No-op if the controller was never created. */
export function notifyMeetingAborted() {
  instance?.handleRecordingAborted();
}

/** Called from the global MeetingFinalized listener when a stopped meeting has
 * been fully drained and saved in the background. */
export function notifyMeetingFinalized(id: string) {
  instance?.handleMeetingFinalized(id);
}

/** The floating pill (or tray) asked to stop the active meeting; run the
 * full stop pipeline so the meeting is saved normally. */
export function notifyMeetingStopRequested() {
  if (instance?.isRecordingMeeting) {
    void instance.stopRecording();
  }
}

/** Called from the global MeetingIdle listener when the backend detects the
 * meeting has probably ended (silence or the max-duration failsafe). */
export function notifyMeetingIdle(payload: MeetingIdle) {
  instance?.handleMeetingIdle(payload);
}

/** Called from the global SystemWokeUp listener and from the webview
 * visibilitychange handler: check for (and offer/auto-start) a meeting
 * that sleep paused. Creates the controller if it doesn't exist yet, since
 * wake can happen before the meeting view has ever mounted. */
export function notifySystemWokeUp() {
  void createMeetingController().resumeAfterSystemWake();
}

// Singleton: survives view mount/unmount cycles so liveMeetingSegments
// and recording state are never lost when the user switches tabs.
let instance: ReturnType<typeof createMeetingControllerInstance> | null = null;

export function createMeetingController() {
  if (!instance) {
    instance = createMeetingControllerInstance();
  }
  return instance;
}

/** Reset the singleton for testing. */
export function resetMeetingControllerForTest() {
  instance = null;
}
