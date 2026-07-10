<script lang="ts">
  import { Accessibility, Check, Mic, Volume2 } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import Spinner from "../../components/ui/Spinner.svelte";
  import {
    getPermissionStatus,
    repairAccessibilityPermission,
    requestPermission,
    type PermissionKind,
  } from "../../api/permissions";
  import type { PermissionStatus, PermState } from "../../types";
  import { errorMessage } from "../../utils";

  let { onClose }: { onClose: () => void } = $props();

  let status = $state<PermissionStatus>({
    microphone: "unknown",
    system_audio: "unknown",
    accessibility: "unknown",
    calendar: "unknown",
  });
  let busy = $state<PermissionKind | null>(null);
  let error = $state("");
  let repairing = $state(false);

  type Row = {
    kind: PermissionKind;
    icon: typeof Mic;
    label: string;
    desc: string;
    action: string;
  };

  const rows = $derived<Row[]>([
    {
      kind: "microphone",
      icon: Mic,
      label: $t("permissions.mic_label"),
      desc: $t("permissions.mic_desc"),
      action: $t("permissions.grant"),
    },
    {
      kind: "system_audio",
      icon: Volume2,
      label: $t("permissions.system_label"),
      desc: $t("permissions.system_desc"),
      action: $t("permissions.grant"),
    },
    {
      kind: "accessibility",
      icon: Accessibility,
      label: $t("permissions.accessibility_label"),
      desc: $t("permissions.accessibility_desc"),
      action: $t("permissions.open_settings"),
    },
  ]);

  function stateOf(kind: PermissionKind): PermState {
    return status[kind];
  }

  async function refreshAll() {
    try {
      status = await getPermissionStatus();
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function grant(kind: PermissionKind) {
    busy = kind;
    error = "";
    try {
      const next = await requestPermission(kind);
      status = { ...status, [kind]: next };
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = null;
    }
  }

  async function repairAccessibility() {
    repairing = true;
    error = "";
    try {
      const next = await repairAccessibilityPermission();
      status = { ...status, accessibility: next };
    } catch (e) {
      error = errorMessage(e);
    } finally {
      repairing = false;
    }
  }

  function finish() {
    try {
      localStorage.setItem("permissionsOnboarded", "1");
    } catch {
      // Private mode / storage disabled — onboarding just shows again next time.
    }
    onClose();
  }

  onMount(() => {
    void refreshAll();
    // Accessibility is granted in System Settings (not via an in-app prompt),
    // so re-check it whenever the window regains focus. Microphone and system
    // audio keep their probed result — re-snapshotting would reset them to
    // "unknown" since snapshot() deliberately doesn't probe.
    const onFocus = () => {
      void getPermissionStatus()
        .then((s) => {
          status = { ...status, accessibility: s.accessibility };
        })
        .catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  });
</script>

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6 backdrop-blur-sm"
  role="dialog"
  aria-modal="true"
  aria-label={$t("permissions.title")}
>
  <div class="surface-card w-full max-w-md flex flex-col gap-5">
    <div class="flex flex-col gap-1">
      <h2 class="font-heading text-lg font-bold">{$t("permissions.title")}</h2>
      <p class="text-sm text-text-muted">{$t("permissions.subtitle")}</p>
    </div>

    <div class="flex flex-col gap-2">
      {#each rows as row (row.kind)}
        {@const s = stateOf(row.kind)}
        <div class="flex flex-col gap-2 rounded-lg bg-surface-1/70 p-3">
          <div class="flex items-center gap-3">
            <row.icon size={20} class="shrink-0 text-text-muted" aria-hidden="true" />
            <div class="min-w-0 flex-1">
              <div class="text-sm font-medium text-text-primary">{row.label}</div>
              <div class="text-xs text-text-muted">{row.desc}</div>
            </div>

            {#if s === "granted"}
              <span class="pill pill-accent inline-flex items-center gap-1">
                <Check size={13} aria-hidden="true" />
                {$t("permissions.granted")}
              </span>
            {:else if s === "unsupported"}
              <span class="pill pill-muted">{$t("permissions.not_supported")}</span>
            {:else}
              <button
                class="btn btn-primary shrink-0 gap-1.5"
                disabled={busy !== null}
                onclick={() => grant(row.kind)}
              >
                {#if busy === row.kind}
                  <Spinner />
                  {$t("permissions.checking")}
                {:else}
                  {row.action}
                {/if}
              </button>
            {/if}
          </div>

          {#if row.kind === "accessibility" && s === "denied"}
            <div class="flex items-center justify-between gap-3 pl-8">
              <p class="text-xs text-text-muted">{$t("permissions.accessibility_stale_hint")}</p>
              <button
                class="btn btn-ghost shrink-0 gap-1.5"
                disabled={repairing || busy !== null}
                onclick={repairAccessibility}
              >
                {#if repairing}
                  <Spinner />
                  {$t("permissions.checking")}
                {:else}
                  {$t("permissions.repair")}
                {/if}
              </button>
            </div>
          {/if}
        </div>
      {/each}
    </div>

    {#if error}
      <p class="text-sm text-red-400">{error}</p>
    {/if}

    <div class="flex items-center justify-between gap-3">
      <span class="text-xs text-text-muted">{$t("permissions.skip_hint")}</span>
      <button class="btn btn-ghost shrink-0" onclick={finish}>
        {$t("permissions.continue")}
      </button>
    </div>
  </div>
</div>
