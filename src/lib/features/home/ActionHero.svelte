<script lang="ts">
  import { Mic, Users } from "@lucide/svelte";
  import { t } from "svelte-i18n";

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

<div class="grid grid-cols-2 gap-3">
  <button
    onclick={onDictate}
    disabled={!modelReady}
    class="surface-card flex cursor-pointer items-center gap-[15px] !p-5 text-left transition-[outline-color,transform,background-color] duration-150 hover:outline-accent/40 active:scale-[0.99] disabled:cursor-default disabled:opacity-50"
  >
    <span class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-accent/13 text-accent">
      <Mic size={20} aria-hidden="true" />
    </span>
    <span class="flex min-w-0 flex-col gap-1">
      <span class="font-heading text-[15px] font-semibold">{$t("home.dictate")}</span>
      <span class="flex items-center gap-[5px] text-[12.5px] text-text-muted">
        {#if dictationShortcut}
          {$t("home.press")}
          <kbd class="rounded-[5px] bg-surface-3 px-1.5 py-[1.5px] font-mono text-[11px] text-text-tertiary">{dictationShortcut}</kbd>
        {:else}
          {$t("home.dictate_hint")}
        {/if}
      </span>
    </span>
  </button>

  <button
    onclick={onMeeting}
    disabled={!modelReady}
    class="surface-card flex cursor-pointer items-center gap-[15px] !p-5 text-left transition-[outline-color,transform,background-color] duration-150 hover:outline-accent/40 active:scale-[0.99] disabled:cursor-default disabled:opacity-50"
  >
    <span class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-secondary/12 text-secondary">
      <Users size={20} aria-hidden="true" />
    </span>
    <span class="flex min-w-0 flex-col gap-1">
      <span class="font-heading text-[15px] font-semibold">{$t("home.meeting")}</span>
      <span class="text-[12.5px] text-text-muted">{$t("home.meeting_hint")}</span>
    </span>
  </button>
</div>
