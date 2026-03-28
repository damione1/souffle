<script lang="ts">
  import { Radio } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import MeetingHistorySection from "../features/meeting/components/MeetingHistorySection.svelte";
  import { createMeetingHistoryController } from "../features/meeting/history-controller.svelte";
  import { getAppState } from "../stores/app.svelte";

  const controller = createMeetingHistoryController();
  const app = getAppState();

  let hasActiveRecording = $derived(app.recordingMode === "meeting");

  onMount(() => {
    void controller.mount();
  });

  function goToActiveRecording() {
    app.currentView = "meeting";
  }
</script>

<div class="flex flex-col gap-4">
  {#if hasActiveRecording}
    <button
      onclick={goToActiveRecording}
      class="flex items-center gap-2.5 px-4 py-3 rounded-default bg-red-500/10 outline-1 outline-red-500/25 text-left cursor-pointer transition-colors hover:bg-red-500/20"
    >
      <Radio size={16} strokeWidth={2} class="text-red-400 animate-pulse shrink-0" />
      <span class="text-sm font-medium text-red-400">{$t("meeting_history.active_recording")}</span>
      <span class="text-xs text-text-muted ml-auto">{$t("meeting_history.click_to_view")}</span>
    </button>
  {/if}

  <MeetingHistorySection
    meetings={controller.meetings}
    filteredMeetings={controller.filteredMeetings}
    statusMessage={controller.statusMessage}
    bind:searchQuery={controller.searchQuery}
    onOpenMeeting={controller.openMeeting}
  />
</div>
