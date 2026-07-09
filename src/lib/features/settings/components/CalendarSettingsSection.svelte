<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { CalendarInfo, PermState } from "../../../types";

  let {
    enabled,
    permission,
    calendars,
    selectedIds,
    reminderMinutes,
    onEnabledChange,
    onToggleCalendar,
    onReminderMinutesChange,
  }: {
    enabled: boolean;
    permission: PermState;
    calendars: CalendarInfo[];
    selectedIds: string[];
    reminderMinutes: number;
    onEnabledChange: (event: Event) => void | Promise<void>;
    onToggleCalendar: (id: string) => void;
    onReminderMinutesChange: (event: Event) => void;
  } = $props();

  let grouped = $derived.by(() => {
    const groups = new Map<string, CalendarInfo[]>();
    for (const calendar of calendars) {
      const key = calendar.source_title ?? "";
      const group = groups.get(key) ?? [];
      group.push(calendar);
      groups.set(key, group);
    }
    return [...groups.entries()];
  });
</script>

<section class="settings-group">
  <h3>{$t("settings_calendar.title")}</h3>
  <div class="settings-rows">
  <SettingsField
    label={$t("settings_calendar.enable_label")}
    description={$t("settings_calendar.enable_desc")}
  >
    {#snippet control()}
      <input type="checkbox" checked={enabled} onchange={onEnabledChange} class="switch" aria-label={$t("settings_calendar.enable_label")} />
    {/snippet}
  </SettingsField>

  {#if permission === "denied"}
    <StatusBanner message={$t("settings_calendar.permission_denied")} />
  {/if}

  {#if enabled && permission === "granted"}
    <SettingsField
      label={$t("settings_calendar.reminder_label")}
      description={$t("settings_calendar.reminder_desc")}
      htmlFor="calendar-reminder-minutes"
    >
      {#snippet control()}
        <input
          id="calendar-reminder-minutes"
          type="number"
          min="1"
          max="30"
          value={reminderMinutes}
          onchange={onReminderMinutesChange}
          class="field-input max-w-20"
        />
      {/snippet}
    </SettingsField>

    {#if calendars.length > 0}
      <div class="flex flex-col gap-1.5">
        <span class="setting-label">{$t("settings_calendar.calendars_label")}</span>
        <p class="setting-desc m-0">{$t("settings_calendar.all_calendars_hint")}</p>
        {#each grouped as [source, group] (source)}
          <div class="mt-1 flex flex-col gap-1">
            {#if source}
              <span class="text-text-muted text-[10.5px] font-semibold uppercase tracking-[0.06em]">{source}</span>
            {/if}
            {#each group as calendar (calendar.id)}
              <label class="flex gap-2 items-center text-sm text-text-secondary">
                <input
                  type="checkbox"
                  checked={selectedIds.length === 0 || selectedIds.includes(calendar.id)}
                  onchange={() => onToggleCalendar(calendar.id)}
                />
                {calendar.title}
              </label>
            {/each}
          </div>
        {/each}
      </div>
    {/if}
  {/if}
  </div>
</section>
