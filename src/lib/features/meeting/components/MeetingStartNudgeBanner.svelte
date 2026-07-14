<script lang="ts">
  import { Play, Video, X } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { MeetingStartNudge } from "../../../types";

  let {
    nudge,
    canStart,
    onStart,
    onDismiss,
  }: {
    nudge: MeetingStartNudge;
    canStart: boolean;
    onStart: () => void;
    onDismiss: () => void;
  } = $props();

  let subtitle = $derived(
    nudge.source === "process" && nudge.app_label
      ? $t("meeting_smart.start_process", { values: { app: nudge.app_label } })
      : nudge.source === "audio_activity"
        ? $t("meeting_smart.start_audio")
        : $t("calendar.started_system_audio"),
  );
</script>

<div class="surface-card flex items-center gap-3 border border-accent/40 px-4 py-3">
  <span class="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-accent/15 text-accent" aria-hidden="true">
    <Video size={16} />
  </span>
  <div class="min-w-0 flex-1">
    <p class="truncate text-sm font-semibold">{nudge.title}</p>
    <p class="text-xs text-text-muted">{subtitle}</p>
  </div>
  <button class="btn btn-primary gap-1.5 shrink-0" disabled={!canStart} onclick={onStart}>
    <Play size={14} />
    {$t("calendar.start_transcription")}
  </button>
  <button class="btn btn-icon shrink-0" onclick={onDismiss} aria-label={$t("calendar.dismiss")}>
    <X size={14} />
  </button>
</div>
