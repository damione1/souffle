<script lang="ts">
  import { onMount } from "svelte";
  import { createTranscriptionController } from "../features/transcription/controller.svelte";
  import HistorySection from "../features/transcription/components/HistorySection.svelte";
  import RecorderSection from "../features/transcription/components/RecorderSection.svelte";
  import StatusHeroSection from "../features/transcription/components/StatusHeroSection.svelte";
  import TranscriptSection from "../features/transcription/components/TranscriptSection.svelte";
  import StatusBanner from "./ui/StatusBanner.svelte";

  const controller = createTranscriptionController();

  onMount(() => {
    let cleanup = () => {};

    void (async () => {
      cleanup = (await controller.mount()) ?? (() => {});
    })();

    return () => cleanup();
  });
</script>

<div class="flex flex-col gap-4">
  {#if controller.statusMessage}
    <StatusBanner message={controller.statusMessage} variant="warning" />
  {/if}

  <StatusHeroSection
    profileLabel={controller.activeProfileLabel}
    runtimePhase={controller.runtimePhase}
    autoPaste={controller.app.settings.auto_paste}
  />

  <div class="grid grid-cols-[minmax(280px,1fr)_minmax(280px,1.2fr)] gap-4 max-[700px]:grid-cols-1">
    <RecorderSection
      isStartingRecording={controller.isStartingRecording}
      isRecording={controller.app.isRecording}
      lockedByMeeting={controller.app.recordingMode === "meeting"}
      runtimePhase={controller.runtimePhase}
      modelOperationState={controller.modelOperationState}
      downloadFile={controller.downloadFile}
      downloadCompletedFiles={controller.downloadCompletedFiles}
      downloadTotalFiles={controller.downloadTotalFiles}
      inputDevice={controller.app.selectedDevice}
      autoPaste={controller.app.settings.auto_paste}
      onDownloadModel={controller.handleDownloadModel}
      onLoadModel={controller.handleLoadModel}
      onToggleRecording={() => controller.toggleRecording()}
    />

    <TranscriptSection
      transcript={controller.transcript}
      isStartingRecording={controller.isStartingRecording}
      isRecording={controller.app.isRecording}
    />
  </div>

  {#if controller.history.length > 0 || controller.historySearchQuery}
    <HistorySection
      history={controller.history}
      filteredHistory={controller.filteredHistory}
      expandedEntryId={controller.expandedEntryId}
      searchResults={controller.historySearchResults}
      bind:searchQuery={controller.historySearchQuery}
      onToggleEntry={(id) => {
        controller.expandedEntryId = controller.expandedEntryId === id ? null : id;
      }}
      onDeleteEntry={controller.removeHistoryEntry}
      onClearHistory={controller.resetHistory}
    />
  {/if}
</div>
