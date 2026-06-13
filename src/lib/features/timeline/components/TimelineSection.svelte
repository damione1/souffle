<script lang="ts">
  import { CalendarClock } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import EmptyState from "../../../components/ui/EmptyState.svelte";
  import type { createTimelineController } from "../controller.svelte";
  import TimelineItem from "./TimelineItem.svelte";

  /** Calendar events for today — populated once calendar sync lands.
   * The section renders above past items, ready for that feature. */
  export interface UpcomingMeeting {
    id: string;
    title: string;
    startsAt: string;
  }

  let {
    controller,
    upcoming = [],
  }: {
    controller: ReturnType<typeof createTimelineController>;
    upcoming?: UpcomingMeeting[];
  } = $props();

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

<div class="flex flex-col gap-5">
  {#if upcoming.length > 0}
    <section class="flex flex-col gap-1.5">
      <h4 class="px-3 text-[0.6875rem] font-semibold uppercase tracking-widest text-text-muted">
        {$t("timeline.upcoming_today")}
      </h4>
      <div class="surface-card flex flex-col gap-1 p-1.5">
        {#each upcoming as event}
          <div class="flex items-center gap-3 rounded-lg px-3 py-2.5">
            <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-emerald-500/15 text-emerald-400" aria-hidden="true">
              <CalendarClock size={14} />
            </span>
            <span class="min-w-0 flex-1 truncate text-sm">{event.title}</span>
            <span class="shrink-0 text-xs tabular-nums text-text-muted">
              {new Date(event.startsAt).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}
            </span>
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
      <section class="flex flex-col gap-1.5">
        <h4 class="px-3 text-[0.6875rem] font-semibold uppercase tracking-widest text-text-muted">
          {dayLabel(group.day)}
        </h4>
        <div class="surface-card flex flex-col p-1.5">
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
