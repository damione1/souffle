<script lang="ts">
  import { Mic, Users } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import { openSettings } from "../settings/open";

  let {
    dictationShortcut,
    modelReady,
    onDictate,
    onMeeting,
  }: {
    dictationShortcut: string;
    modelReady: boolean;
    onDictate: () => void;
    onMeeting: () => void;
  } = $props();
</script>

<div class="grid grid-cols-2 gap-0">
  <button
    onclick={dictationShortcut ? onDictate : () => openSettings({ anchor: "interface.shortcuts" })}
    disabled={!modelReady && !!dictationShortcut}
    class="-ml-px flex cursor-pointer items-center gap-[15px] p-[18px] text-left outline-1 -outline-offset-1 outline-ghost-border transition-[outline-color,transform] duration-150 first:ml-0 hover:outline-accent/40 active:scale-[0.99] disabled:cursor-default disabled:opacity-50"
  >
    <span class="flex h-[26px] w-[26px] shrink-0 items-center justify-center text-accent">
      <Mic size={22} aria-hidden="true" />
    </span>
    <span class="flex min-w-0 flex-col gap-1">
      <span class="font-heading text-[15px] font-semibold">{$t("home.dictate")}</span>
      <span class="flex items-center gap-[5px] text-[12.5px] text-text-muted">
        {#if dictationShortcut}
          {$t("home.press")}
          <kbd class="bg-surface-2 px-1.5 py-[1.5px] font-mono text-[11px] text-text-tertiary">{dictationShortcut}</kbd>
        {:else}
          {$t("home.dictate_hint")}
        {/if}
      </span>
    </span>
  </button>

  <button
    onclick={onMeeting}
    disabled={!modelReady}
    class="-ml-px flex cursor-pointer items-center gap-[15px] p-[18px] text-left outline-1 -outline-offset-1 outline-ghost-border transition-[outline-color,transform] duration-150 first:ml-0 hover:outline-accent/40 active:scale-[0.99] disabled:cursor-default disabled:opacity-50"
  >
    <span class="flex h-[26px] w-[26px] shrink-0 items-center justify-center text-secondary">
      <Users size={22} aria-hidden="true" />
    </span>
    <span class="flex min-w-0 flex-col gap-1">
      <span class="font-heading text-[15px] font-semibold">{$t("home.meeting")}</span>
      <span class="text-[12.5px] text-text-muted">{$t("home.meeting_hint")}</span>
    </span>
  </button>
</div>
