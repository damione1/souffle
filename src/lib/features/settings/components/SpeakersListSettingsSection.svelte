<script lang="ts">
  import { onMount } from "svelte";
  import { Pencil } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import ConfirmAction from "../../../components/ui/ConfirmAction.svelte";
  import { deleteSpeaker, listSpeakerProfiles, renameSpeaker } from "../../../api/speakers";
  import type { SpeakerProfile } from "../../../types";

  let profiles = $state<SpeakerProfile[]>([]);
  let error = $state("");
  let editingId = $state<number | null>(null);
  let editName = $state("");

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
          <li class="flex items-center justify-between gap-3 py-1.5">
            <div class="min-w-0 flex-1">
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
              <p class="setting-desc m-0 px-1.5">
                {$t("settings_speakers.list_meta", {
                  values: {
                    date: formatLastSeen(profile.last_seen_at),
                    count: profile.meeting_count,
                  },
                })}
              </p>
            </div>
            <ConfirmAction
              label={$t("settings_speakers.delete")}
              confirmLabel={$t("settings_speakers.delete_confirm")}
              confirmMessage={$t("settings_speakers.delete_msg")}
              variant="danger"
              onConfirm={() => void handleDelete(profile.id)}
            />
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</section>
