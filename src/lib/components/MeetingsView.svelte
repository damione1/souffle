<script lang="ts">
  import { Plus } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import MeetingDetail from "../features/meeting/components/MeetingDetail.svelte";
  import MeetingHistorySection from "../features/meeting/components/MeetingHistorySection.svelte";
  import { createMeetingController } from "../features/meeting/controller.svelte";
  import { createMeetingHistoryController } from "../features/meeting/history-controller.svelte";
  import StatusBanner from "./ui/StatusBanner.svelte";

  const controller = createMeetingController();
  const list = createMeetingHistoryController();
  const app = controller.app;

  const showDetail = $derived(
    Boolean(controller.meeting) || controller.isLoadingMeeting || Boolean(app.currentMeetingId),
  );
  const lockedByDictation = $derived(app.recordingMode === "dictation");
  const modelNotReady = $derived(app.transcriptionRuntimePhase !== "ready");
  const canStart = $derived(!lockedByDictation && !modelNotReady && !controller.isRecordingMeeting);

  onMount(() => {
    void controller.mount();
    void list.mount();
  });

  $effect(() => {
    void controller.onMeetingSelectionChange(app.currentMeetingId);
  });

  // Refresh the list whenever we come back from a detail view (a meeting
  // may have been recorded, renamed, or deleted there).
  $effect(() => {
    if (!showDetail) {
      void list.mount();
    }
  });
</script>

{#if showDetail}
  <MeetingDetail {controller} />
{:else}
  <div class="flex flex-col gap-4">
    {#if controller.statusMessage}
      <StatusBanner message={controller.statusMessage} variant="warning" />
    {/if}

    <div class="flex items-center justify-between gap-3 flex-wrap">
      <h2>{$t("meetings.title")}</h2>
      <button
        onclick={() => void controller.startRecording()}
        disabled={!canStart}
        class="btn btn-primary gap-2"
        title={lockedByDictation
          ? $t("meetings.locked_by_dictation")
          : modelNotReady
            ? $t("meetings.model_not_ready")
            : ""}
      >
        <Plus size={16} aria-hidden="true" />
        {$t("meetings.new_button")}
      </button>
    </div>

    {#if lockedByDictation}
      <p class="text-sm text-text-muted">{$t("meetings.locked_by_dictation")}</p>
    {/if}

    <MeetingHistorySection
      meetings={list.meetings}
      filteredMeetings={list.filteredMeetings}
      statusMessage={list.statusMessage}
      searchResults={list.searchResults}
      isSearching={list.isSearching}
      bind:searchQuery={list.searchQuery}
      onOpenMeeting={list.openMeeting}
    />
  </div>
{/if}
