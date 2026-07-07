<script lang="ts">
  import { Square } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import Waveform from "../../components/Waveform.svelte";
  import Spinner from "../../components/ui/Spinner.svelte";
  import { groupIntoParagraphs } from "../../utils/paragraphs";
  import MeetingNotesSection from "../meeting/components/MeetingNotesSection.svelte";
  import type { createMeetingController } from "../meeting/controller.svelte";
  import type { createTranscriptionController } from "../transcription/controller.svelte";

  let {
    mode,
    transcription,
    meeting,
  }: {
    mode: "dictation" | "meeting";
    transcription: ReturnType<typeof createTranscriptionController>;
    meeting: ReturnType<typeof createMeetingController>;
  } = $props();

  let elapsedSeconds = $state(0);
  let transcriptEl: HTMLDivElement | undefined = $state();

  const liveText = $derived(mode === "dictation" ? transcription.transcript : "");

  // Meetings render through the same paragraph grouping as the finished
  // transcript, so diarized sessions show Me/Them live: segments are ordered
  // by time and split on speaker changes instead of one undifferentiated blob.
  const liveParagraphs = $derived(
    mode === "meeting" ? groupIntoParagraphs(meeting.liveMeetingSegments, 1.5) : [],
  );

  const hasLiveContent = $derived(mode === "dictation" ? Boolean(liveText) : liveParagraphs.length > 0);

  const elapsed = $derived(
    `${Math.floor(elapsedSeconds / 60)}:${`${elapsedSeconds % 60}`.padStart(2, "0")}`,
  );

  const stopping = $derived(
    mode === "dictation" ? transcription.isStopping : meeting.isStopping,
  );

  function stop() {
    if (stopping) return;
    if (mode === "dictation") {
      void transcription.toggleRecording();
    } else {
      void meeting.stopRecording();
    }
  }

  // Keep the latest words in view as they stream in.
  $effect(() => {
    void liveText;
    void liveParagraphs;
    if (transcriptEl) transcriptEl.scrollTop = transcriptEl.scrollHeight;
  });

  onMount(() => {
    const timer = setInterval(() => {
      elapsedSeconds += 1;
    }, 1000);
    return () => clearInterval(timer);
  });
</script>

<section class="surface-card flex flex-col gap-4 outline-red-500/25">
  <div class="flex items-center gap-3">
    <span class="pill pill-danger inline-flex items-center gap-1.5">
      <span class="recording-dot"></span>
      {mode === "meeting" ? $t("home.live_meeting") : $t("home.live_dictation")}
    </span>
    {#if mode === "meeting" && meeting.app.systemAudioStatus}
      <span class="pill pill-muted" title={meeting.app.systemAudioStatus.reason ?? ""}>
        {meeting.app.systemAudioStatus.active
          ? $t("meeting_header.system_audio_active")
          : $t("meeting_header.system_audio_unavailable")}
      </span>
    {/if}
    <div class="min-w-0 flex-1">
      <Waveform active variant="pill" />
    </div>
    <span class="shrink-0 text-sm tabular-nums text-text-muted">{elapsed}</span>
    <button onclick={stop} disabled={stopping} class="btn btn-danger gap-1.5">
      {#if stopping}
        <Spinner />
        {$t("home.stopping")}
      {:else}
        <Square size={14} aria-hidden="true" />
        {$t("home.stop")}
      {/if}
    </button>
  </div>

  <div class={`grid gap-3 ${mode === "meeting" ? "grid-cols-2 max-[700px]:grid-cols-1" : ""}`}>
    <div
      bind:this={transcriptEl}
      class="max-h-44 min-h-24 overflow-y-auto rounded-lg bg-surface-1/70 p-3 text-sm leading-relaxed text-text-secondary"
    >
      {#if !hasLiveContent}
        <span class="text-text-muted">{$t("home.listening")}</span>
      {:else if mode === "dictation"}
        {liveText}
      {:else}
        {#each liveParagraphs as paragraph}
          <p class="mb-2 last:mb-0">
            {#if paragraph.speaker}
              <span
                class="mr-1 text-xs font-semibold"
                class:text-accent={paragraph.speaker === "me"}
                class:text-text-primary={paragraph.speaker === "them"}
              >{paragraph.speaker === "me" ? $t("transcript.me") : $t("transcript.them")}</span>
            {/if}
            {paragraph.text}
          </p>
        {/each}
      {/if}
    </div>

    {#if mode === "meeting"}
      <MeetingNotesSection
        notes={meeting.notesDraft}
        saveState={meeting.notesSaveState}
        onNotesChange={meeting.onNotesChange}
      />
    {/if}
  </div>
</section>
