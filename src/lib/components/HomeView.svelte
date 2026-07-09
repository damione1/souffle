<script lang="ts">
  import { Search, Settings } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import { getShortcuts } from "../api/settings";
  import { formatShortcutLabel } from "../utils";
  import ActionHero from "../features/home/ActionHero.svelte";
  import LiveSessionCard from "../features/home/LiveSessionCard.svelte";
  import { createCalendarController } from "../features/calendar/controller.svelte";
  import UpcomingMeetingBanner from "../features/calendar/components/UpcomingMeetingBanner.svelte";
  import MeetingDetail from "../features/meeting/components/MeetingDetail.svelte";
  import { createMeetingController } from "../features/meeting/controller.svelte";
  import { createTimelineController } from "../features/timeline/controller.svelte";
  import TimelineSection from "../features/timeline/components/TimelineSection.svelte";
  import { createTranscriptionController } from "../features/transcription/controller.svelte";
  import StatusBanner from "./ui/StatusBanner.svelte";
  import StatusChip from "./ui/StatusChip.svelte";

  const app = createTimelineController().app;
  const timeline = createTimelineController();
  const transcription = createTranscriptionController();
  const meeting = createMeetingController();
  const calendar = createCalendarController();

  let dictationShortcut = $state("");

  const recordingMode = $derived(app.recordingMode);
  // While recording, the live card on the home screen is the surface;
  // the meeting detail only takes over once the session has stopped.
  const showMeetingDetail = $derived(
    recordingMode === "idle"
      && (Boolean(meeting.meeting) || meeting.isLoadingMeeting || Boolean(app.currentMeetingId)),
  );
  const modelReady = $derived(app.transcriptionRuntimePhase === "ready");

  onMount(() => {
    void timeline.refresh();
    void meeting.mount();
    void calendar.refresh();
    getShortcuts()
      .then((shortcuts) => {
        dictationShortcut = formatShortcutLabel(shortcuts.toggle);
      })
      .catch(() => {});
  });

  $effect(() => {
    void meeting.onMeetingSelectionChange(app.currentMeetingId);
  });

  // The meeting controller probes Ollama once on mount. If the user later
  // starts Ollama and connects in Settings, re-probe when the sheet closes so
  // the summary section reflects the new state instead of staying on
  // "Connect Ollama in Settings".
  let settingsWasOpen = false;
  $effect(() => {
    const open = app.settingsOpen;
    if (settingsWasOpen && !open) {
      void meeting.checkOllama();
      // Calendar integration or selection may have changed in Settings.
      void calendar.refresh();
    }
    settingsWasOpen = open;
  });

  // Refresh the timeline whenever a recording ends or a detail closes.
  $effect(() => {
    if (recordingMode === "idle" && !showMeetingDetail) {
      void timeline.refresh();
      void calendar.refresh();
    }
  });
</script>

<div class="mx-auto flex h-full w-full max-w-[720px] flex-col gap-[26px]">
  {#if showMeetingDetail}
    <MeetingDetail controller={meeting} />
  {:else}
    {#if transcription.statusMessage}
      <StatusBanner message={transcription.statusMessage} variant="warning" />
    {/if}
    {#if meeting.statusMessage}
      <StatusBanner message={meeting.statusMessage} variant="warning" />
    {/if}

    {#if recordingMode === "idle"}
      {#if app.upcomingMeeting}
        <UpcomingMeetingBanner
          reminder={app.upcomingMeeting}
          canStart={modelReady}
          onStart={() => {
            const reminder = app.upcomingMeeting;
            app.upcomingMeeting = null;
            if (reminder) void calendar.startFromEvent(reminder.event);
          }}
          onDismiss={() => {
            app.upcomingMeeting = null;
          }}
        />
      {/if}

      <ActionHero
        {dictationShortcut}
        {modelReady}
        onDictate={() => void transcription.toggleRecording()}
        onMeeting={() => void meeting.startRecording()}
      />

      <label class="flex items-center gap-[11px] rounded-xl bg-input px-[15px] py-[11px] outline-1 outline-ghost-border focus-within:outline-accent/50">
        <Search size={16} class="shrink-0 text-text-muted" aria-hidden="true" />
        <input
          type="text"
          bind:value={timeline.searchQuery}
          placeholder={$t("home.search_placeholder")}
          class="w-full bg-transparent text-[13.5px] text-text-secondary outline-none placeholder:text-text-muted"
        />
      </label>

      {#if timeline.statusMessage}
        <StatusBanner message={timeline.statusMessage} />
      {/if}
      {#if calendar.statusMessage}
        <StatusBanner message={calendar.statusMessage} variant="warning" />
      {/if}

      <TimelineSection
        controller={timeline}
        upcoming={calendar.events}
        canStartEvent={modelReady}
        onStartEvent={(event) => void calendar.startFromEvent(event)}
      />
    {:else}
      <!-- During a live session, the session card is the only focus. -->
      <LiveSessionCard
        mode={recordingMode === "meeting" ? "meeting" : "dictation"}
        {transcription}
        {meeting}
      />
    {/if}
  {/if}
</div>
