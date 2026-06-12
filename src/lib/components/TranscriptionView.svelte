<script lang="ts">
  import { createTranscriptionController } from "../features/transcription/controller.svelte";
  import DictationHero from "../features/transcription/components/DictationHero.svelte";
  import HistorySection from "../features/transcription/components/HistorySection.svelte";
  import TranscriptSection from "../features/transcription/components/TranscriptSection.svelte";
  import StatusBanner from "./ui/StatusBanner.svelte";
  import StatusChip from "./ui/StatusChip.svelte";

  // The controller is a singleton mounted by App.svelte (shortcut listeners
  // must outlive this view).
  const controller = createTranscriptionController();
</script>

<div class="flex flex-col gap-4">
  {#if controller.statusMessage}
    <StatusBanner message={controller.statusMessage} variant="warning" />
  {/if}

  <div class="flex justify-end">
    <StatusChip
      phase={controller.runtimePhase}
      operationState={controller.modelOperationState}
      downloadedBytes={controller.downloadedBytes}
      downloadTotalBytes={controller.downloadTotalBytes}
    />
  </div>

  <!-- Reserved slot: calendar-detected meeting suggestion card will land
       here once calendar sync exists ({#if suggestion} … {/if}). -->

  <div class="grid grid-cols-[minmax(280px,1fr)_minmax(280px,1.2fr)] gap-4 max-[700px]:grid-cols-1">
    <DictationHero
      isStartingRecording={controller.isStartingRecording}
      isRecording={controller.app.isRecording}
      lockedByMeeting={controller.app.recordingMode === "meeting"}
      runtimePhase={controller.runtimePhase}
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
