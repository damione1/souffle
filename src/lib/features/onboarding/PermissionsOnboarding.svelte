<script lang="ts">
  import { t } from "svelte-i18n";
  import PermissionsStep from "./PermissionsStep.svelte";
  import { markPermissionsDone } from "./setup";

  let { onClose }: { onClose: () => void } = $props();

  function finish() {
    markPermissionsDone();
    onClose();
  }
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

    <PermissionsStep />

    <div class="flex items-center justify-between gap-3">
      <span class="text-xs text-text-muted">{$t("permissions.skip_hint")}</span>
      <button class="btn btn-ghost shrink-0" onclick={finish}>
        {$t("permissions.continue")}
      </button>
    </div>
  </div>
</div>
