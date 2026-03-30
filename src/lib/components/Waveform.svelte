<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  let { active = false }: { active?: boolean } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let animationId = 0;
  let bars: number[] = [];
  let audioLevel = 0;
  const BAR_COUNT = 48;
  const BAR_WIDTH = 3;
  const BAR_GAP = 2;

  for (let i = 0; i < BAR_COUNT; i++) {
    bars.push(0.15 + Math.random() * 0.1);
  }

  // Poll backend for audio RMS level when recording is active
  let pollInterval: ReturnType<typeof setInterval> | null = null;

  $effect(() => {
    if (active) {
      pollInterval = setInterval(async () => {
        try {
          audioLevel = await invoke<number>("get_audio_level");
        } catch {
          audioLevel = 0;
        }
      }, 50); // ~20Hz polling
    } else {
      if (pollInterval) clearInterval(pollInterval);
      pollInterval = null;
      audioLevel = 0;
    }

    return () => {
      if (pollInterval) clearInterval(pollInterval);
    };
  });

  onMount(() => {
    const ctx = canvas?.getContext("2d");
    if (!ctx || !canvas) return;

    function draw() {
      if (!ctx || !canvas) return;
      const { width, height } = canvas;
      ctx.clearRect(0, 0, width, height);

      const totalBarWidth = BAR_COUNT * (BAR_WIDTH + BAR_GAP) - BAR_GAP;
      const offsetX = (width - totalBarWidth) / 2;

      for (let i = 0; i < BAR_COUNT; i++) {
        if (active) {
          // Drive bars from real audio level with per-bar variation
          const variation = Math.sin(Date.now() / 200 + i * 0.5) * 0.15;
          const spread = Math.sin(i * 0.3 + Date.now() / 300) * 0.1;
          const target = Math.max(0.05, audioLevel + variation + spread);
          bars[i] += (target - bars[i]) * 0.3;
          bars[i] = Math.max(0.05, Math.min(1, bars[i]));
        } else {
          // Idle: subtle ambient sine wave
          const target = 0.12 + Math.sin(Date.now() / 800 + i * 0.3) * 0.08;
          bars[i] += (target - bars[i]) * 0.08;
        }

        const barHeight = bars[i] * (height - 4);
        const x = offsetX + i * (BAR_WIDTH + BAR_GAP);
        const y = (height - barHeight) / 2;

        ctx.fillStyle = active
          ? `rgba(78, 142, 255, ${0.4 + bars[i] * 0.6})`
          : `rgba(78, 142, 255, ${0.15 + bars[i] * 0.2})`;
        ctx.beginPath();
        ctx.roundRect(x, y, BAR_WIDTH, barHeight, 1.5);
        ctx.fill();
      }

      animationId = requestAnimationFrame(draw);
    }

    function resize() {
      if (!canvas) return;
      const rect = canvas.getBoundingClientRect();
      // Setting canvas.width/height resets the context state (including
      // transforms), so the subsequent scale() call is not cumulative.
      canvas.width = rect.width * window.devicePixelRatio;
      canvas.height = rect.height * window.devicePixelRatio;
      ctx!.scale(window.devicePixelRatio, window.devicePixelRatio);
    }

    resize();
    window.addEventListener("resize", resize);
    draw();

    return () => {
      cancelAnimationFrame(animationId);
      window.removeEventListener("resize", resize);
    };
  });
</script>

<div class="w-full h-10 shrink-0 border-t border-ghost-border bg-surface-1">
  <canvas bind:this={canvas} class="block w-full h-full"></canvas>
</div>
