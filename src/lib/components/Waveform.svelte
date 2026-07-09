<script lang="ts">
  import { onMount } from "svelte";
  import { events } from "../api/generated";

  let {
    active = false,
    variant = "footer",
  }: {
    active?: boolean;
    variant?: "footer" | "inline" | "pill";
  } = $props();

  const wrapperClass = $derived(
    {
      footer: "w-full h-10 shrink-0 border-t border-ghost-border bg-surface-1",
      inline: "w-full h-12 shrink-0",
      pill: "w-full h-8 shrink-0",
    }[variant],
  );

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

  // Reset to silence whenever this instance stops being the active recorder,
  // so a stale level doesn't linger on the idle ambient animation.
  $effect(() => {
    if (!active) audioLevel = 0;
  });

  onMount(() => {
    const ctx = canvas?.getContext("2d");
    if (!ctx || !canvas) return;

    // The backend pushes AudioLevel while any capture session is active.
    // Only apply it here when this instance is the one recording (the main
    // window and the pill window can both mount a Waveform).
    let unlistenAudioLevel: (() => void) | undefined;
    events.audioLevel.listen((event) => {
      if (active) audioLevel = event.payload.level;
    }).then((fn) => {
      unlistenAudioLevel = fn;
    });

    // Resolve the themed accent color for the bars; refreshed periodically so
    // a theme toggle mid-session recolors the waveform.
    let accent = "#e9ae55";
    function refreshAccent() {
      if (!canvas) return;
      const value = getComputedStyle(canvas).getPropertyValue("--color-accent").trim();
      if (value) accent = value;
    }
    refreshAccent();
    let accentTimer: ReturnType<typeof setInterval> | null = null;

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

        ctx.fillStyle = accent;
        ctx.globalAlpha = active ? 0.4 + bars[i] * 0.6 : 0.15 + bars[i] * 0.2;
        ctx.beginPath();
        ctx.roundRect(x, y, BAR_WIDTH, barHeight, 1.5);
        ctx.fill();
        ctx.globalAlpha = 1;
      }

      // Stop rescheduling while the document is hidden instead of drawing
      // into an invisible canvas 60 times a second.
      if (document.hidden) {
        animationId = 0;
        return;
      }
      animationId = requestAnimationFrame(draw);
    }

    function startLoops() {
      if (accentTimer === null) accentTimer = setInterval(refreshAccent, 1000);
      if (animationId === 0) draw();
    }

    function stopLoops() {
      if (accentTimer !== null) {
        clearInterval(accentTimer);
        accentTimer = null;
      }
      if (animationId !== 0) {
        cancelAnimationFrame(animationId);
        animationId = 0;
      }
    }

    function handleVisibilityChange() {
      if (document.hidden) stopLoops();
      else startLoops();
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
    document.addEventListener("visibilitychange", handleVisibilityChange);
    if (!document.hidden) startLoops();

    return () => {
      stopLoops();
      unlistenAudioLevel?.();
      window.removeEventListener("resize", resize);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  });
</script>

<div class={wrapperClass}>
  <canvas bind:this={canvas} class="block w-full h-full"></canvas>
</div>
