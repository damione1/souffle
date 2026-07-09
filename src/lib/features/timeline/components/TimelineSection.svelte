<script lang="ts">
  import { CalendarClock, Play, Users } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import EmptyState from "../../../components/ui/EmptyState.svelte";
  import type { CalendarEvent } from "../../../types";
  import type { createTimelineController } from "../controller.svelte";
  import TimelineItem from "./TimelineItem.svelte";

  let {
    controller,
    upcoming = [],
    canStartEvent = false,
    onStartEvent,
  }: {
    controller: ReturnType<typeof createTimelineController>;
    /** Today's calendar events, in start order. */
    upcoming?: CalendarEvent[];
    canStartEvent?: boolean;
    onStartEvent?: (event: CalendarEvent) => void;
  } = $props();

  // Drives the now/next/past styling of today's events.
  let now = $state(Date.now());
  $effect(() => {
    if (upcoming.length === 0) return;
    const timer = setInterval(() => {
      now = Date.now();
    }, 30_000);
    return () => clearInterval(timer);
  });

  const occurrenceKey = (event: CalendarEvent) => `${event.id}-${event.start}`;

  let firstUpcomingKey = $derived(
    upcoming
      .filter((event) => Date.parse(event.start) > now)
      .map(occurrenceKey)[0] ?? null,
  );

  type EventPhase = "past" | "now" | "next" | "later";
  function eventPhase(event: CalendarEvent): EventPhase {
    if (now >= Date.parse(event.end)) return "past";
    if (now >= Date.parse(event.start)) return "now";
    return occurrenceKey(event) === firstUpcomingKey ? "next" : "later";
  }

  const timeLabel = (iso: string) =>
    new Date(iso).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

  function dayLabel(day: string): string {
    const todayKey = new Date();
    const yesterday = new Date(Date.now() - 86_400_000);
    const toKey = (date: Date) =>
      `${date.getFullYear()}-${`${date.getMonth() + 1}`.padStart(2, "0")}-${`${date.getDate()}`.padStart(2, "0")}`;
    if (day === toKey(todayKey)) return $t("timeline.today");
    if (day === toKey(yesterday)) return $t("timeline.yesterday");
    return new Date(`${day}T12:00:00`).toLocaleDateString(undefined, {
      weekday: "long",
      day: "numeric",
      month: "long",
    });
  }
</script>

<div class="flex flex-col gap-[26px]">
  {#if upcoming.length > 0}
    <section class="flex flex-col gap-2.5">
      <h4 class="px-1 text-[11px] font-semibold uppercase tracking-[0.15em] text-text-muted">
        {$t("timeline.upcoming_today")}
      </h4>
      <div class="surface-card flex flex-col gap-0.5 !p-1.5">
        {#each upcoming as event (occurrenceKey(event))}
          {@const phase = eventPhase(event)}
          <div
            class="flex items-center gap-3 rounded-[11px] px-3 py-[11px] transition-colors hover:bg-surface-2 {phase === 'past'
              ? 'opacity-50'
              : ''} {phase === 'now' ? 'bg-accent/7' : ''}"
          >
            <span class="flex h-[30px] w-[30px] shrink-0 items-center justify-center rounded-[9px] bg-secondary/12 text-secondary" aria-hidden="true">
              <CalendarClock size={15} />
            </span>
            <span class="flex min-w-0 flex-1 items-center gap-[9px]">
              <span class="truncate text-[13.5px] text-text-primary">{event.title}</span>
              {#if phase === "now"}
                <span class="shrink-0 rounded-full bg-accent/13 px-2 py-0.5 text-[10.5px] font-semibold uppercase tracking-[0.06em] text-accent">{$t("calendar.now")}</span>
              {:else if phase === "next"}
                <span class="shrink-0 rounded-full bg-surface-2 px-2 py-0.5 text-[10.5px] font-semibold uppercase tracking-[0.06em] text-text-muted">{$t("calendar.next")}</span>
              {/if}
            </span>
            {#if event.participants.length > 0}
              <span class="flex shrink-0 items-center gap-[5px] text-xs text-text-muted" title={event.participants.map((p) => p.name).join(", ")}>
                <Users size={13} aria-hidden="true" />
                {event.participants.length}
              </span>
            {/if}
            <span class="shrink-0 font-mono text-[11.5px] text-text-muted">
              {timeLabel(event.start)}–{timeLabel(event.end)}
            </span>
            {#if onStartEvent && phase !== "past"}
              <button
                class="flex h-[30px] w-[30px] shrink-0 cursor-pointer items-center justify-center rounded-[9px] bg-accent/10 text-accent outline-1 outline-accent/25 transition-colors hover:bg-accent/20 disabled:cursor-default disabled:opacity-50"
                disabled={!canStartEvent}
                onclick={() => onStartEvent(event)}
                aria-label={$t("calendar.start_transcription")}
                title={$t("calendar.start_transcription")}
              >
                <Play size={13} fill="currentColor" />
              </button>
            {/if}
          </div>
        {/each}
      </div>
    </section>
  {/if}

  {#if controller.isEmpty}
    <EmptyState
      title={$t("timeline.empty_title")}
      message={$t("timeline.empty_msg")}
    />
  {:else if !controller.hasMatches}
    <EmptyState
      title={$t("timeline.no_matches_title")}
      message={$t("timeline.no_matches_msg")}
    />
  {:else}
    {#each controller.groups as group (group.day)}
      <section class="flex flex-col gap-2.5">
        <h4 class="px-1 text-[11px] font-semibold uppercase tracking-[0.15em] text-text-muted">
          {dayLabel(group.day)}
        </h4>
        <div class="surface-card flex flex-col gap-0.5 !p-1.5">
          {#each group.items as item (item.kind + item.id)}
            <TimelineItem
              {item}
              expanded={item.kind === "dictation" && controller.expandedDictationId === item.id}
              onOpen={() => controller.openItem(item)}
              onRemove={() => void controller.removeItem(item)}
            />
          {/each}
        </div>
      </section>
    {/each}
  {/if}
</div>
