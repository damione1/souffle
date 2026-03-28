<script lang="ts">
  import { t } from "svelte-i18n";
  import type { TranscriptionRuntimePhase } from "../../../types";
  import { runtimePhaseHeroLabelKey, runtimePhasePillClass } from "../state";

  let {
    profileLabel,
    runtimePhase,
    autoPaste,
  }: {
    profileLabel: string;
    runtimePhase: TranscriptionRuntimePhase;
    autoPaste: boolean;
  } = $props();
</script>

<section class="surface-card">
  <div class="flex items-center justify-between gap-3 flex-wrap">
    <div class="flex flex-wrap gap-2">
      <span class="pill pill-blue">{profileLabel}</span>
      <span class={`pill ${runtimePhasePillClass(runtimePhase)}`}>
        {$t(runtimePhaseHeroLabelKey(runtimePhase))}
      </span>
      <span class={`pill ${autoPaste ? "pill-blue" : "pill-muted"}`}>
        {autoPaste ? $t("status_hero.auto_paste") : $t("status_hero.manual_copy")}
      </span>
    </div>
  </div>
</section>
