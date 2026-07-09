<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { DictationPolishTemplate } from "../../../types";

  const builtInLabelKeys: Record<string, string> = {
    email: "settings_dictation_polish.template_email",
    bullets: "settings_dictation_polish.template_bullets",
    no_fillers: "settings_dictation_polish.template_no_fillers",
  };

  let {
    enabled,
    templateId,
    templates,
    providerAvailable,
    onEnabledChange,
    onTemplateChange,
    onPromptChange,
  }: {
    enabled: boolean;
    templateId: string;
    templates: DictationPolishTemplate[];
    providerAvailable: boolean;
    onEnabledChange: (event: Event) => void;
    onTemplateChange: (event: Event) => void;
    onPromptChange: (event: Event) => void;
  } = $props();

  let activeTemplate = $derived(
    templates.find((template) => template.id === templateId) ?? templates[0] ?? null,
  );

  function templateLabel(template: DictationPolishTemplate): string {
    const key = builtInLabelKeys[template.id];
    return key ? $t(key) : template.label;
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_dictation_polish.title")}</h3>
  <p class="text-sm text-text-muted mb-3">{$t("settings_dictation_polish.description")}</p>
  <div class="settings-rows">
    <SettingsField
      label={$t("settings_dictation_polish.enabled")}
      description={$t("settings_dictation_polish.enabled_desc")}
    >
      {#snippet control()}
        <input
          type="checkbox"
          checked={enabled}
          onchange={onEnabledChange}
          class="switch"
          aria-label={$t("settings_dictation_polish.enabled")}
        />
      {/snippet}
    </SettingsField>

    {#if enabled && !providerAvailable}
      <StatusBanner message={$t("settings_dictation_polish.no_provider")} />
    {/if}

    {#if enabled && templates.length > 0}
      <div class="flex items-center justify-between gap-4">
        <label for="dictation-polish-template" class="setting-label shrink-0">
          {$t("settings_dictation_polish.template")}
        </label>
        <select
          id="dictation-polish-template"
          value={templateId}
          onchange={onTemplateChange}
          class="field-select max-w-64"
        >
          {#each templates as template (template.id)}
            <option value={template.id}>{templateLabel(template)}</option>
          {/each}
        </select>
      </div>

      {#if activeTemplate}
        <SettingsField
          label={$t("settings_dictation_polish.prompt")}
          description={$t("settings_dictation_polish.prompt_desc")}
          htmlFor="dictation-polish-prompt"
        >
          {#snippet control()}
            <textarea
              id="dictation-polish-prompt"
              value={activeTemplate.prompt}
              onchange={onPromptChange}
              rows="4"
              class="field-input min-h-24 w-full max-w-xl"
            ></textarea>
          {/snippet}
        </SettingsField>
      {/if}
    {/if}
  </div>
</section>
