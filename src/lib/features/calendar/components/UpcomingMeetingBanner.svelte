<script lang="ts">
  import { CalendarClock, Play, X } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { UpcomingMeeting } from "../../../types";

  let {
    reminder,
    canStart,
    onStart,
    onDismiss,
  }: {
    reminder: UpcomingMeeting;
    canStart: boolean;
    onStart: () => void;
    onDismiss: () => void;
  } = $props();

  // The banner goes stale on its own: 10 minutes after the event started,
  // there is nothing left to offer.
  $effect(() => {
    const expiry = Date.parse(reminder.event.start) + 10 * 60_000 - Date.now();
    if (expiry <= 0) {
      onDismiss();
      return;
    }
    const timer = setTimeout(onDismiss, expiry);
    return () => clearTimeout(timer);
  });

  let minutesLabel = $derived(Math.max(1, Math.ceil(reminder.starts_in_seconds / 60)));
</script>

<div class="surface-card flex items-center gap-3 border border-accent/40 px-4 py-3">
  <span class="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-accent/15 text-accent" aria-hidden="true">
    <CalendarClock size={16} />
  </span>
  <div class="min-w-0 flex-1">
    <p class="truncate text-sm font-semibold">{reminder.event.title}</p>
    <p class="text-xs text-text-muted">
      {$t("calendar.starts_in_minutes", { values: { minutes: minutesLabel } })}
    </p>
  </div>
  <button class="btn btn-primary gap-1.5 shrink-0" disabled={!canStart} onclick={onStart}>
    <Play size={14} />
    {$t("calendar.start_transcription")}
  </button>
  <button class="btn btn-icon shrink-0" onclick={onDismiss} aria-label={$t("calendar.dismiss")}>
    <X size={14} />
  </button>
</div>
