<script lang="ts">
  import { onMount } from "svelte";
  import MeetingHistorySection from "../features/meeting/components/MeetingHistorySection.svelte";
  import { createMeetingHistoryController } from "../features/meeting/history-controller.svelte";

  const controller = createMeetingHistoryController();

  onMount(() => {
    void controller.mount();
  });
</script>

<MeetingHistorySection
  meetings={controller.meetings}
  filteredMeetings={controller.filteredMeetings}
  statusMessage={controller.statusMessage}
  bind:searchQuery={controller.searchQuery}
  onOpenMeeting={controller.openMeeting}
/>
