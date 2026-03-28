<script lang="ts">
  let {
    meetingTitle,
    lockedByDictation,
    onMeetingTitleChange,
    onStartRecording,
  }: {
    meetingTitle: string;
    lockedByDictation: boolean;
    onMeetingTitleChange: (value: string) => void;
    onStartRecording: () => void | Promise<void>;
  } = $props();
</script>

<div class="flex flex-col items-center justify-center h-full gap-6">
  <input
    type="text"
    value={meetingTitle}
    placeholder="Meeting title (optional)"
    class="field-input w-full max-w-sm text-center"
    disabled={lockedByDictation}
    oninput={(event) => onMeetingTitleChange((event.currentTarget as HTMLInputElement).value)}
    onkeydown={(event) => {
      if (event.key === "Enter" && !lockedByDictation) {
        void onStartRecording();
      }
    }}
  />
  <button onclick={onStartRecording} disabled={lockedByDictation} class="btn btn-primary btn-lg">
    Start Recording
  </button>
  {#if lockedByDictation}
    <p class="text-sm text-text-muted">Stop the dictation before starting a meeting.</p>
  {:else}
    <p class="text-sm text-text-muted">Leave empty to use the current date</p>
  {/if}
</div>
