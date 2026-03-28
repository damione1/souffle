<script lang="ts">
  import type { TranscriptionRuntimePhase } from "../../../types";
  import { runtimePhaseHeroLabel, runtimePhasePillClass } from "../state";

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
        {runtimePhaseHeroLabel(runtimePhase)}
      </span>
      <span class={`pill ${autoPaste ? "pill-blue" : "pill-muted"}`}>
        {autoPaste ? "Auto-paste" : "Manual copy"}
      </span>
    </div>
  </div>
</section>
