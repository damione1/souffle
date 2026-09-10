<script lang="ts">
  import { t } from "svelte-i18n";
  import { addDictionaryEntry } from "../../api/dictionary";
  import { getSettings, saveSettings } from "../../api/settings";

  const { onComplete } = $props<{ onComplete: () => void }>();

  let qIndex = $state(0);
  let answer = $state("");
  let saving = $state(false);
  let polishEnabled = $state(true);

  const questions = [
    { id: "name", labelKey: "onboarding.interview_q_name" },
    { id: "job", labelKey: "onboarding.interview_q_job" },
    { id: "people", labelKey: "onboarding.interview_q_people" },
    { id: "jargon", labelKey: "onboarding.interview_q_jargon" },
    { id: "tone", labelKey: "onboarding.interview_q_tone" },
  ];

  $effect(() => {
    getSettings().then((s) => {
      polishEnabled = s.dictation_polish_enabled;
    });
  });

  async function handleNext(skipped: boolean) {
    if (saving) return;
    saving = true;

    if (!skipped && answer.trim()) {
      const q = questions[qIndex];
      if (q.id === "tone") {
        const s = await getSettings();
        const newTemplate = {
          id: `custom_${Date.now()}`,
          label: "Style personnalisé",
          prompt: answer.trim(),
        };
        s.dictation_polish_templates.push(newTemplate);
        s.dictation_polish_template_id = newTemplate.id;
        s.dictionary_interview_done = true;
        await saveSettings(s);
      } else {
        const terms = answer.split(/[,;]+/).map(t => t.trim()).filter(Boolean);
        for (const term of terms) {
          try {
            await addDictionaryEntry(term, null, "interview");
          } catch (e) {
            // Ignore unique constraint or other errors silently as requested
          }
        }
      }
    }

    if (qIndex === questions.length - 1) {
      if (!skipped || answer.trim() === "") {
         const s = await getSettings();
         s.dictionary_interview_done = true;
         await saveSettings(s);
      }
      onComplete();
    } else {
      qIndex++;
      answer = "";
      saving = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleNext(false);
    }
  }
</script>

<div class="flex flex-col gap-4 text-left">
  <label for="interview-answer" class="field-label">{$t(questions[qIndex].labelKey)}</label>
  
  <input
    id="interview-answer"
    type="text"
    class="field-input"
    bind:value={answer}
    onkeydown={handleKeydown}
    disabled={saving}
  />

  {#if questions[qIndex].id === "tone" && !polishEnabled}
    <p class="text-xs text-warning bg-warning/10 p-2 rounded">
      {$t("onboarding.interview_ton_polish_warning")}
    </p>
  {/if}

  <div class="flex justify-end gap-2 mt-2">
    <button
      type="button"
      class="btn btn-ghost"
      onclick={() => handleNext(true)}
      disabled={saving}
    >
      {$t("onboarding.interview_skip")}
    </button>
    <button
      type="button"
      class="btn btn-primary"
      onclick={() => handleNext(false)}
      disabled={saving || !answer.trim()}
    >
      {$t("onboarding.interview_next")}
    </button>
  </div>
</div>
