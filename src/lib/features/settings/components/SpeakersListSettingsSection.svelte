<script lang="ts">
  import { onMount } from "svelte";
  import { Pencil } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import ConfirmAction from "../../../components/ui/ConfirmAction.svelte";
  import {
    deleteSpeaker,
    listSpeakerProfiles,
    mergeSpeakers,
    renameSpeaker,
    setSpeakerIsMe,
  } from "../../../api/speakers";
  import type { SpeakerProfile } from "../../../types";

  let profiles = $state<SpeakerProfile[]>([]);
  let error = $state("");
  let editingId = $state<number | null>(null);
  let editName = $state("");
  let mergingId = $state<number | null>(null);
  let mergeTargetId = $state<number | null>(null);

  async function refresh() {
    try {
      profiles = await listSpeakerProfiles();
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    void refresh();
  });

  function startEditing(profile: SpeakerProfile) {
    editingId = profile.id;
    editName = profile.name;
  }

  async function saveName(id: number) {
    const name = editName.trim();
    editingId = null;
    if (!name) return;
    try {
      await renameSpeaker(id, name);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function handleDelete(id: number) {
    try {
      await deleteSpeaker(id);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function handleSetIsMe(id: number, isMe: boolean) {
    try {
      await setSpeakerIsMe(id, isMe);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function startMerge(profile: SpeakerProfile) {
    mergingId = profile.id;
    mergeTargetId = profiles.find((candidate) => candidate.id !== profile.id)?.id ?? null;
  }

  function cancelMerge() {
    mergingId = null;
    mergeTargetId = null;
  }

  async function handleMerge(sourceId: number, targetId: number | null) {
    if (targetId == null) return;
    try {
      await mergeSpeakers(sourceId, targetId);
      cancelMerge();
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function formatLastSeen(iso: string): string {
    const date = new Date(iso);
    return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString();
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_speakers.list_title")}</h3>
  <div class="settings-rows">
    <p class="setting-desc m-0">{$t("settings_speakers.list_desc")}</p>

    {#if error}
      <p class="text-sm text-danger-strong m-0">{error}</p>
    {/if}

    {#if profiles.length === 0 && !error}
      <p class="setting-desc m-0">{$t("settings_speakers.list_empty")}</p>
    {:else}
      <ul class="m-0 flex list-none flex-col gap-1 p-0">
        {#each profiles as profile (profile.id)}
          <li class="flex flex-col gap-1.5 py-1.5">
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-1.5">
                  {#if editingId === profile.id}
                    <!-- svelte-ignore a11y_autofocus -->
                    <input
                      type="text"
                      class="field-input"
                      bind:value={editName}
                      autofocus
                      onblur={() => void saveName(profile.id)}
                      onkeydown={(event) => {
                        if (event.key === "Enter") (event.currentTarget as HTMLInputElement).blur();
                        if (event.key === "Escape") {
                          editName = profile.name;
                          (event.currentTarget as HTMLInputElement).blur();
                        }
                      }}
                      aria-label={$t("settings_speakers.rename_aria", { values: { name: profile.name } })}
                    />
                  {:else}
                    <button
                      class="btn btn-ghost gap-[7px] px-1.5 py-1 text-[13px] font-medium"
                      onclick={() => startEditing(profile)}
                      title={$t("settings_speakers.rename_hint")}
                    >
                      <span class="truncate">{profile.name}</span>
                      <Pencil size={13} aria-hidden="true" />
                    </button>
                  {/if}
                  {#if profile.is_me}
                    <span class="pill pill-accent">
                      {$t("settings_speakers.me_badge")}
                    </span>
                  {/if}
                </div>
                <p class="setting-desc m-0 px-1.5">
                  {$t("settings_speakers.list_meta", {
                    values: {
                      date: formatLastSeen(profile.last_seen_at),
                      count: profile.meeting_count,
                    },
                  })}
                </p>
              </div>
              <div class="flex shrink-0 items-center gap-1.5">
                <button
                  class="btn btn-ghost px-2.5 py-1.5 text-[12.5px]"
                  onclick={() => void handleSetIsMe(profile.id, !profile.is_me)}
                >
                  {profile.is_me ? $t("settings_speakers.unmark_me") : $t("settings_speakers.mark_me")}
                </button>
                {#if profiles.length > 1 && mergingId !== profile.id}
                  <button
                    class="btn btn-ghost px-2.5 py-1.5 text-[12.5px]"
                    onclick={() => startMerge(profile)}
                  >
                    {$t("settings_speakers.merge")}
                  </button>
                {/if}
                <ConfirmAction
                  label={$t("settings_speakers.delete")}
                  confirmLabel={$t("settings_speakers.delete_confirm")}
                  confirmMessage={$t("settings_speakers.delete_msg")}
                  variant="danger"
                  onConfirm={() => void handleDelete(profile.id)}
                />
              </div>
            </div>

            {#if mergingId === profile.id}
              <div class="flex flex-wrap items-center gap-2 rounded-[9px] bg-surface-2 px-2.5 py-2">
                <span class="text-[12.5px] text-text-secondary">
                  {$t("settings_speakers.merge_into", { values: { name: profile.name } })}
                </span>
                <select
                  bind:value={mergeTargetId}
                  class="field-select max-w-[180px] text-[12.5px]"
                  aria-label={$t("settings_speakers.merge_into", { values: { name: profile.name } })}
                >
                  {#each profiles.filter((candidate) => candidate.id !== profile.id) as candidate (candidate.id)}
                    <option value={candidate.id}>{candidate.name}</option>
                  {/each}
                </select>
                <ConfirmAction
                  label={$t("settings_speakers.merge")}
                  confirmLabel={$t("settings_speakers.merge_confirm")}
                  confirmMessage={$t("settings_speakers.merge_msg", {
                    values: {
                      source: profile.name,
                      target: profiles.find((candidate) => candidate.id === mergeTargetId)?.name ?? "",
                    },
                  })}
                  variant="danger"
                  onConfirm={() => void handleMerge(profile.id, mergeTargetId)}
                />
                <button class="btn btn-ghost px-2.5 py-1.5 text-[12.5px]" onclick={cancelMerge}>
                  {$t("ui.cancel")}
                </button>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</section>
