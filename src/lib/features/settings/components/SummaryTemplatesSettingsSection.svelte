<script lang="ts">
  import { Plus, Trash2 } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import type { SummaryTemplate } from "../../../types";

  const builtInIds = ["default", "detailed_minutes", "brief_overview"];
  const builtInNameKeys: Record<string, string> = {
    default: "summary_templates.template_default",
    detailed_minutes: "summary_templates.template_detailed_minutes",
    brief_overview: "summary_templates.template_brief_overview",
  };

  let {
    templates,
    defaultTemplateId,
    onDefaultChange,
    onNameChange,
    onPromptChange,
    onAdd,
    onDelete,
  }: {
    templates: SummaryTemplate[];
    defaultTemplateId: string;
    onDefaultChange: (templateId: string) => void;
    onNameChange: (templateId: string, name: string) => void;
    onPromptChange: (templateId: string, prompt: string) => void;
    onAdd: (name: string) => void;
    onDelete: (templateId: string) => void;
  } = $props();

  let editingTemplateId = $state("");
  let newTemplateName = $state("");

  let editingTemplate = $derived(
    templates.find((template) => template.id === editingTemplateId)
    ?? templates.find((template) => template.id === defaultTemplateId)
    ?? templates[0]
    ?? null,
  );

  function isBuiltIn(id: string): boolean {
    return builtInIds.includes(id);
  }

  function templateName(template: SummaryTemplate): string {
    const key = builtInNameKeys[template.id];
    return key ? $t(key) : template.name;
  }

  function handleAdd() {
    const name = newTemplateName.trim();
    if (!name) return;
    onAdd(name);
    newTemplateName = "";
  }
</script>

<section class="settings-group">
  <h3>{$t("summary_templates.title")}</h3>
  <p class="text-sm text-text-muted mb-3">{$t("summary_templates.description")}</p>
  <div class="settings-rows">
    <div class="flex items-center justify-between gap-4">
      <label for="summary-template-default" class="setting-label shrink-0">
        {$t("summary_templates.default")}
      </label>
      <select
        id="summary-template-default"
        value={defaultTemplateId}
        onchange={(event) => onDefaultChange((event.currentTarget as HTMLSelectElement).value)}
        class="field-select max-w-64"
      >
        {#each templates as template (template.id)}
          <option value={template.id}>{templateName(template)}</option>
        {/each}
      </select>
    </div>

    {#if editingTemplate}
      {@const tpl = editingTemplate}
      <div class="flex items-center justify-between gap-4">
        <label for="summary-template-edit" class="setting-label shrink-0">
          {$t("summary_templates.edit")}
        </label>
        <div class="flex items-center gap-1.5">
          <select
            id="summary-template-edit"
            value={tpl.id}
            onchange={(event) => { editingTemplateId = (event.currentTarget as HTMLSelectElement).value; }}
            class="field-select max-w-64"
          >
            {#each templates as template (template.id)}
              <option value={template.id}>{templateName(template)}</option>
            {/each}
          </select>
          {#if !isBuiltIn(tpl.id)}
            <button
              onclick={() => { editingTemplateId = ""; onDelete(tpl.id); }}
              class="btn btn-icon btn-ghost text-text-muted hover:!text-danger-soft"
              aria-label={`${$t("summary_templates.delete")} ${templateName(tpl)}`}
            >
              <Trash2 size={15} />
            </button>
          {/if}
        </div>
      </div>

      {#if !isBuiltIn(tpl.id)}
        <SettingsField
          label={$t("summary_templates.name")}
          htmlFor="summary-template-name"
        >
          {#snippet control()}
            <input
              id="summary-template-name"
              type="text"
              value={tpl.name}
              onchange={(event) => onNameChange(tpl.id, (event.currentTarget as HTMLInputElement).value)}
              class="field-input w-64"
            />
          {/snippet}
        </SettingsField>
      {/if}

      <SettingsField
        label={$t("summary_templates.prompt")}
        description={isBuiltIn(tpl.id)
          ? $t("summary_templates.prompt_desc_builtin")
          : $t("summary_templates.prompt_desc")}
        htmlFor="summary-template-prompt"
      >
        {#snippet control()}
          <textarea
            id="summary-template-prompt"
            value={tpl.prompt}
            onchange={(event) => onPromptChange(tpl.id, (event.currentTarget as HTMLTextAreaElement).value)}
            rows="6"
            class="field-input min-h-32 w-full max-w-xl font-mono text-xs"
          ></textarea>
        {/snippet}
      </SettingsField>
    {/if}

    <div class="flex items-end gap-1.5">
      <div class="flex flex-col gap-1 flex-1 max-w-64">
        <label for="summary-template-new" class="field-label">{$t("summary_templates.new_name")}</label>
        <input
          id="summary-template-new"
          type="text"
          bind:value={newTemplateName}
          onkeydown={(event) => { if (event.key === "Enter") handleAdd(); }}
          placeholder={$t("summary_templates.new_name_placeholder")}
          class="field-input"
        />
      </div>
      <button
        onclick={handleAdd}
        class="btn btn-primary btn-icon"
        aria-label={$t("summary_templates.add")}
        disabled={!newTemplateName.trim()}
      >
        <Plus size={16} />
      </button>
    </div>
  </div>
</section>
