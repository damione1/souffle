<script lang="ts">
  let {
    meetingTitle,
    onMeetingTitleChange,
    onStartRecording,
  }: {
    meetingTitle: string;
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
    oninput={(event) => onMeetingTitleChange((event.currentTarget as HTMLInputElement).value)}
    onkeydown={(event) => {
      if (event.key === "Enter") {
        void onStartRecording();
      }
    }}
  />
  <button onclick={onStartRecording} class="btn btn-primary btn-lg">
    Start Recording
  </button>
  <p class="text-sm text-text-muted">Leave empty to use the current date</p>
</div>
