<script lang="ts">
  import { t } from "svelte-i18n";

  let {
    meetingTitle,
    lockedByDictation,
    modelNotReady,
    onMeetingTitleChange,
    onStartRecording,
  }: {
    meetingTitle: string;
    lockedByDictation: boolean;
    modelNotReady: boolean;
    onMeetingTitleChange: (value: string) => void;
    onStartRecording: () => void | Promise<void>;
  } = $props();

  let disabled = $derived(lockedByDictation || modelNotReady);
</script>

<div class="flex flex-col items-center justify-center h-full gap-6">
  <input
    type="text"
    value={meetingTitle}
    placeholder={$t("new_meeting.title_placeholder")}
    class="field-input w-full max-w-sm text-center"
    {disabled}
    oninput={(event) => onMeetingTitleChange((event.currentTarget as HTMLInputElement).value)}
    onkeydown={(event) => {
      if (event.key === "Enter" && !disabled) {
        void onStartRecording();
      }
    }}
  />
  <button onclick={onStartRecording} {disabled} class="btn btn-primary btn-lg">
    {$t("new_meeting.start_recording")}
  </button>
  {#if lockedByDictation}
    <p class="text-sm text-text-muted">{$t("new_meeting.locked_by_dictation")}</p>
  {:else if modelNotReady}
    <p class="text-sm text-text-muted">{$t("new_meeting.model_not_ready")}</p>
  {:else}
    <p class="text-sm text-text-muted">{$t("new_meeting.empty_title_hint")}</p>
  {/if}
</div>
