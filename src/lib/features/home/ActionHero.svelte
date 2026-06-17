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

<div class="grid grid-cols-2 gap-2">
  <button
    onclick={onDictate}
    disabled={!modelReady}
    class="surface-card group flex cursor-pointer items-center gap-4 !p-5 text-left transition-[outline-color,transform] duration-150 hover:outline-accent/40 active:scale-[0.99] disabled:cursor-default disabled:opacity-50"
  >
    <span class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-accent/15 text-accent transition-colors group-hover:bg-accent group-hover:text-white">
      <Mic size={20} aria-hidden="true" />
    </span>
    <span class="flex min-w-0 flex-col">
      <span class="font-heading text-base font-semibold">{$t("home.dictate")}</span>
      <span class="text-xs text-text-muted">
        {#if dictationShortcut}
          <kbd class="rounded bg-surface-3 px-1.5 py-0.5 font-sans text-[0.6875rem]">{dictationShortcut}</kbd>
        {:else}
          {$t("home.dictate_hint")}
        {/if}
      </span>
    </span>
  </button>

  <button
    onclick={onMeeting}
    disabled={!modelReady}
    class="surface-card group flex cursor-pointer items-center gap-4 !p-5 text-left transition-[outline-color,transform] duration-150 hover:outline-accent/40 active:scale-[0.99] disabled:cursor-default disabled:opacity-50"
  >
    <span class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-sage/15 text-sage transition-colors group-hover:bg-sage group-hover:text-white">
      <Users size={20} aria-hidden="true" />
    </span>
    <span class="flex min-w-0 flex-col">
      <span class="font-heading text-base font-semibold">{$t("home.meeting")}</span>
      <span class="text-xs text-text-muted">{$t("home.meeting_hint")}</span>
    </span>
  </button>
</div>
