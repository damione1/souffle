<script lang="ts">
  import { t } from "svelte-i18n";
  import type { MeetingSpeaker } from "../../../types";

  let {
    speakerId,
    speakerName,
    meetingSpeakers,
    allSpeakers,
    onClose,
    onRename,
    onRetag,
  }: {
    speakerId: number;
    speakerName: string;
    meetingSpeakers: MeetingSpeaker[];
    allSpeakers: MeetingSpeaker[];
    onClose: () => void;
    onRename: (name: string) => void | Promise<void>;
    onRetag: (options: {
      scope: "turn" | "meeting";
      toSpeakerId: number | null;
      newSpeakerName: string | null;
    }) => void | Promise<void>;
  } = $props();

  let nameDraft = $state("");
  let retagScope = $state<"turn" | "meeting">("turn");
  let targetMode = $state<"existing" | "new">("existing");
  let targetSpeakerId = $state<number | null>(null);
  let newSpeakerName = $state("");
  let isSaving = $state(false);

  $effect(() => {
    nameDraft = speakerName;
    retagScope = "turn";
    targetMode = "existing";
    newSpeakerName = "";
  });

  const pickerSpeakers = $derived.by(() => {
    const seen = new Set<number>();
    const merged: MeetingSpeaker[] = [];
    for (const speaker of [...meetingSpeakers, ...allSpeakers]) {
      if (speaker.id === speakerId || seen.has(speaker.id)) continue;
      seen.add(speaker.id);
      merged.push(speaker);
    }
    merged.sort((a, b) => a.name.localeCompare(b.name));
    return merged;
  });

  $effect(() => {
    if (targetSpeakerId == null && pickerSpeakers.length > 0) {
      targetSpeakerId = pickerSpeakers[0].id;
    }
  });

  async function saveRename() {
    const trimmed = nameDraft.trim();
    if (!trimmed || trimmed === speakerName) {
      onClose();
      return;
    }
    isSaving = true;
    try {
      await onRename(trimmed);
      onClose();
    } finally {
      isSaving = false;
    }
  }

  async function applyRetag() {
    isSaving = true;
    try {
      if (targetMode === "existing") {
        if (targetSpeakerId == null) return;
        await onRetag({
          scope: retagScope,
          toSpeakerId: targetSpeakerId,
          newSpeakerName: null,
        });
      } else {
        const trimmed = newSpeakerName.trim();
        if (!trimmed) return;
        await onRetag({
          scope: retagScope,
          toSpeakerId: null,
          newSpeakerName: trimmed,
        });
      }
      onClose();
    } finally {
      isSaving = false;
    }
  }
</script>

<button
  type="button"
  class="fixed inset-0 z-10 cursor-default"
  aria-label={$t("ui.cancel")}
  onclick={onClose}
></button>

<div class="absolute left-0 top-full z-20 mt-1.5 w-72 rounded-[11px] bg-surface-1 p-3 shadow-lg outline-1 outline-ghost-border">
  <p class="m-0 mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-text-muted">
    {$t("speaker_manage.rename_heading")}
  </p>
  <div class="flex gap-2">
    <input
      bind:value={nameDraft}
      class="field-input flex-1 text-[13px]"
      aria-label={$t("speaker_manage.rename_label")}
      onkeydown={(e) => {
        if (e.key === "Enter") void saveRename();
        if (e.key === "Escape") onClose();
      }}
    />
    <button class="btn px-2.5 py-1 text-[12.5px]" disabled={isSaving} onclick={() => void saveRename()}>
      {$t("speaker_manage.save_name")}
    </button>
  </div>

  <div class="my-3 h-px bg-ghost-border"></div>

  <p class="m-0 mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-text-muted">
    {$t("speaker_manage.retag_heading")}
  </p>

  <fieldset class="m-0 mb-2 flex flex-col gap-1.5 border-0 p-0">
    <label class="flex items-center gap-2 text-[12.5px] text-text-secondary">
      <input type="radio" bind:group={retagScope} value="turn" />
      {$t("speaker_manage.scope_turn")}
    </label>
    <label class="flex items-center gap-2 text-[12.5px] text-text-secondary">
      <input type="radio" bind:group={retagScope} value="meeting" />
      {$t("speaker_manage.scope_meeting")}
    </label>
  </fieldset>

  <div class="mb-2 flex flex-col gap-1.5">
    <label class="flex items-center gap-2 text-[12.5px] text-text-secondary">
      <input type="radio" bind:group={targetMode} value="existing" />
      {$t("speaker_manage.pick_existing")}
    </label>
    {#if targetMode === "existing"}
      <select bind:value={targetSpeakerId} class="field-select text-[12.5px]" disabled={pickerSpeakers.length === 0}>
        {#if pickerSpeakers.length === 0}
          <option value={null}>{$t("speaker_manage.no_other_speakers")}</option>
        {:else}
          {#each pickerSpeakers as speaker (speaker.id)}
            <option value={speaker.id}>{speaker.name}</option>
          {/each}
        {/if}
      </select>
    {/if}

    <label class="flex items-center gap-2 text-[12.5px] text-text-secondary">
      <input type="radio" bind:group={targetMode} value="new" />
      {$t("speaker_manage.new_speaker")}
    </label>
    {#if targetMode === "new"}
      <input
        bind:value={newSpeakerName}
        class="field-input text-[13px]"
        placeholder={$t("speaker_manage.new_speaker_placeholder")}
        aria-label={$t("speaker_manage.new_speaker_placeholder")}
      />
    {/if}
  </div>

  <button
    class="btn btn-primary w-full text-[12.5px]"
    disabled={isSaving || (targetMode === "existing" && pickerSpeakers.length === 0)}
    onclick={() => void applyRetag()}
  >
    {$t("speaker_manage.apply_retag")}
  </button>
</div>
