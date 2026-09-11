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
  import { openSettings } from "../settings/open";
  import { getAppState } from "../../stores/app.svelte";

  let { onStatusChange }: { onStatusChange?: (status: PermissionStatus) => void } = $props();

  const app = getAppState();

  let status = $state<PermissionStatus>({
    microphone: "unknown",
    system_audio: "unknown",
    accessibility: "unknown",
    calendar: "unknown",
  });
  let busy = $state<Record<string, boolean>>({});
  let error = $state("");
  let repairing = $state(false);
  let repairSuccess = $state(false);
  let repairCooldown = $state(false);
  let repairCooldownTimer: ReturnType<typeof setTimeout> | undefined;

  /**
   * The "stale TCC entry" diagnosis is only plausible once the user has tried
   * to grant Accessibility and come back with it still refused. On a fresh
   * install nothing was ever granted: there is no stale entry, no row to
   * remove with the minus button, and the Repair it advertises resets an
   * entry that does not exist, so it can only fail (SOU-055).
   *
   * Two independent triggers, because the click on Open Settings alone is not
   * one: `request_permission` returns Denied synchronously while System
   * Settings is still opening. A blur/focus pair is the honest signal ("you
   * went there and came back without it"); the attempt count is the fallback
   * for a webview that never sees one, so Repair stays reachable either way.
   */
  let accessibilityAttempts = $state(0);
  let leftAfterAttempt = $state(false);
  let returnedStillDenied = $state(false);
  let leftWindow = $state(false);
  const showStaleHint = $derived(returnedStillDenied || accessibilityAttempts >= 2);

  /** Every write to `status` goes through here so the parent (which cannot
   * see this component's local state otherwise) learns the real permission
   * state, e.g. to gate the onboarding auto-paste default (SOU-053). */
  function setStatus(next: PermissionStatus) {
    status = next;
    // Publish upward: this component already polls TCC every 600 ms, so the
    // app-level snapshot rides on it rather than starting a second poll.
    app.appPermissions = next;
    // Reset success banner if accessibility state changes back/forth
    if (next.accessibility === "granted") {
      repairSuccess = false;
      accessibilityAttempts = 0;
      leftAfterAttempt = false;
      returnedStillDenied = false;
    }
    onStatusChange?.(next);
  }

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
      setStatus(await getPermissionStatus());
    } catch (e) {
      error = errorMessage(e);
    }
  }

  /**
   * Requests permission for a given capability, tracking the busy state per-kind.
   */
  async function grant(kind: PermissionKind) {
    busy[kind] = true;
    error = "";
    if (kind === "accessibility") accessibilityAttempts += 1;
    try {
      const next = await requestPermission(kind);
      setStatus({ ...status, [kind]: next });
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy[kind] = false;
    }
  }

  /**
   * Triggers the accessibility repair flow.
   */
  async function repairAccessibility() {
    repairing = true;
    repairSuccess = false;
    error = "";
    try {
      await repairAccessibilityPermission();
      // Don't overwrite accessibility with Denied: a successful reset plus
      // prompt cannot observe the grant yet (SOU-054). The 600 ms poll
      // is what turns the row green once TCC reports granted.
      repairSuccess = true;
      repairCooldown = true;
      clearTimeout(repairCooldownTimer);
      repairCooldownTimer = setTimeout(() => {
        repairCooldown = false;
      }, 2500);
    } catch (e) {
      error = errorMessage(e);
      repairSuccess = false;
    } finally {
      repairing = false;
    }
  }

  onMount(() => {
    void refreshAll();
    // Poll TCC permissions every 600ms so the UI updates live without requiring
    // the user to switch focus back and forth (focus churn). Skip a tick
    // while a request is in flight so a slow snapshot cannot overwrite a
    // newer one.
    let pollInFlight = false;
    const timer = setInterval(() => {
      if (pollInFlight || repairing || Object.values(busy).some(Boolean)) return;
      pollInFlight = true;
      void getPermissionStatus()
        .then((s) => {
          // A request in flight is newer than this snapshot: drop the
          // result rather than letting it overwrite a fresher answer.
          if (Object.values(busy).some(Boolean)) return;
          // Snapshot never prompts. Unknown means "not determined"; keep
          // the local value rather than wiping a grant in progress.
          const next = { ...status };
          if (s.accessibility !== "unknown") next.accessibility = s.accessibility;
          if (s.microphone !== "unknown") next.microphone = s.microphone;
          if (s.system_audio !== "unknown") next.system_audio = s.system_audio;
          if (s.calendar !== "unknown") next.calendar = s.calendar;

          if (
            next.accessibility !== status.accessibility ||
            next.microphone !== status.microphone ||
            next.system_audio !== status.system_audio ||
            next.calendar !== status.calendar
          ) {
            setStatus(next);
          }
        })
        .catch(() => {})
        .finally(() => {
          pollInFlight = false;
        });
    }, 600);

    // "Went to System Settings and came back without it" is the only moment
    // the stale-entry diagnosis is worth showing. Tracked as a pair so a
    // window that regains focus without ever having lost it (the synchronous
    // Denied that `request_permission` returns) does not count as a return.
    const onBlur = () => {
      if (accessibilityAttempts > 0) leftAfterAttempt = true;
      leftWindow = true;
    };
    const onFocus = () => {
      if (leftAfterAttempt && status.accessibility === "denied") {
        returnedStillDenied = true;
      }
      // Re-probe a remembered grant after the user left the app (typically
      // System Settings) so a revoke is reflected. Opening the window does
      // not probe (AC5). Denied already has Grant; Unknown must not prompt
      // (AC3). Skip while a probe is in flight so two taps cannot overlap.
      if (
        leftWindow
        && status.system_audio === "granted"
        && !busy.system_audio
      ) {
        leftWindow = false;
        void grant("system_audio");
      }
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);

    return () => {
      clearInterval(timer);
      clearTimeout(repairCooldownTimer);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
    };
  });
</script>

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
            disabled={busy[row.kind]}
            onclick={() => grant(row.kind)}
          >
            {#if busy[row.kind]}
              <Spinner />
              {$t("permissions.checking")}
            {:else}
              {row.action}
            {/if}
          </button>
        {/if}
      </div>

      {#if row.kind === "accessibility" && s === "denied"}
        <div class="pl-8">
          <p class="text-xs text-text-muted">
            {#if showStaleHint}
              {$t("permissions.accessibility_stale_hint")}
            {:else}
              {$t("permissions.accessibility_denied_hint")}
            {/if}
          </p>
        </div>
      {:else if row.kind === "microphone" && s === "denied"}
        <div class="flex items-center justify-between gap-3 pl-8">
          <p class="text-xs text-text-muted">{$t("permissions.mic_denied_hint")}</p>
          <button
            class="btn btn-ghost shrink-0 gap-1.5"
            disabled={busy[row.kind]}
            onclick={() => grant(row.kind)}
          >
            {#if busy[row.kind]}
              <Spinner />
              {$t("permissions.checking")}
            {:else}
              {$t("permissions.open_settings")}
            {/if}
          </button>
        </div>
      {:else if row.kind === "microphone" && s === "no_device"}
        <div class="flex items-center gap-3 pl-8">
          <p class="text-xs text-text-muted">{$t("permissions.mic_no_device_hint")}</p>
          <button
            class="btn btn-ghost shrink-0 gap-1.5"
            onclick={() => openSettings({ anchor: "audio.mic" })}
          >
            {$t("permissions.open_settings")}
          </button>
        </div>
      {/if}
    </div>
  {/each}

  <div class="flex items-center justify-between gap-3 px-1 pt-1">
    <p class="text-xs text-text-muted">
      {#if repairSuccess}
        {$t("permissions.accessibility_repair_success")}
      {:else}
        {$t("permissions.issues_prompt")}
      {/if}
    </p>
    <button
      class="btn btn-ghost shrink-0 gap-1.5"
      disabled={repairing || repairCooldown}
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
</div>

{#if error}
  <p class="text-sm text-red-400">{error}</p>
{/if}
