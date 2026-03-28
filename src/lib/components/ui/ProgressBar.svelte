<script lang="ts">
  let {
    value,
    max = 100,
    label = "",
  }: {
    value: number;
    max?: number;
    label?: string;
  } = $props();

  let safeMax = $derived(Math.max(1, max));
  let safeValue = $derived(Math.max(0, Math.min(value, safeMax)));
  let percent = $derived(Math.round((safeValue / safeMax) * 100));
</script>

<div class="flex flex-col gap-1.5">
  <div
    class="h-2 w-full overflow-hidden rounded-full bg-surface-2 outline-1 outline-ghost-border"
    role="progressbar"
    aria-valuemin={0}
    aria-valuemax={safeMax}
    aria-valuenow={safeValue}
    aria-label={label || "Progress"}
  >
    <div
      class="h-full rounded-full bg-accent transition-[width] duration-200 ease-out"
      style={`width: ${percent}%`}
    ></div>
  </div>
  <div class="flex items-center justify-between gap-3 text-xs text-text-muted">
    <span>{label}</span>
    <span>{percent}%</span>
  </div>
</div>
