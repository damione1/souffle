<script lang="ts">
  import { ShieldCheck } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import PermissionsOnboarding from "../../onboarding/PermissionsOnboarding.svelte";

  let showPanel = $state(false);
</script>

<section class="settings-group">
  <h3>{$t("permissions.settings_title")}</h3>
  <div class="settings-rows">
    <SettingsField
      label={$t("permissions.settings_title")}
      description={$t("permissions.settings_desc")}
    >
      {#snippet control()}
        <button
          class="btn btn-ghost gap-1.5 px-2.5 py-[5px] text-[12.5px]"
          onclick={() => (showPanel = true)}
        >
          <ShieldCheck size={14} aria-hidden="true" />
          {$t("permissions.review")}
        </button>
      {/snippet}
    </SettingsField>
  </div>
</section>

{#if showPanel}
  <PermissionsOnboarding onClose={() => (showPanel = false)} />
{/if}
