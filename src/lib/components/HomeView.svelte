<script lang="ts">
  import { Search, Settings } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import { getShortcuts } from "../api/settings";
  import { formatShortcutLabel } from "../utils";
  import ActionHero from "../features/home/ActionHero.svelte";
  import LiveSessionCard from "../features/home/LiveSessionCard.svelte";
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
    getShortcuts()
      .then((shortcuts) => {
        dictationShortcut = formatShortcutLabel(shortcuts.toggle);
      })
      .catch(() => {});
  });

  $effect(() => {
    void meeting.onMeetingSelectionChange(app.currentMeetingId);
  });

  // Refresh the timeline whenever a recording ends or a detail closes.
  $effect(() => {
    if (recordingMode === "idle" && !showMeetingDetail) {
      void timeline.refresh();
    }
  });
</script>

<div class="mx-auto flex h-full w-full max-w-3xl flex-col gap-5">
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
      <ActionHero
        {dictationShortcut}
        {modelReady}
        onDictate={() => void transcription.toggleRecording()}
        onMeeting={() => void meeting.startRecording()}
      />

      <label class="flex items-center gap-2.5 rounded-xl bg-surface-2/70 px-3.5 py-2.5 outline-1 outline-ghost-border focus-within:outline-accent/50">
        <Search size={15} class="shrink-0 text-text-muted" aria-hidden="true" />
        <input
          type="text"
          bind:value={timeline.searchQuery}
          placeholder={$t("home.search_placeholder")}
          class="w-full bg-transparent text-sm outline-none placeholder:text-text-muted"
        />
      </label>

      {#if timeline.statusMessage}
        <StatusBanner message={timeline.statusMessage} />
      {/if}

      <TimelineSection controller={timeline} />
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
