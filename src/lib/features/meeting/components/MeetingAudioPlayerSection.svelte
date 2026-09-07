<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { Pause, Play } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import { defaultAudioTarget, buildPlayCommand, type AudioSeekTarget } from "../audio-map";
  import { applyPendingSeek, seekNeedsMetadata, type PendingSeek } from "../audio-seek";
  import type { MeetingAudioSession } from "../../../types";
  import { formatTimestamp } from "../../../utils";

  let {
    audioSessions,
    seekTarget,
    seekRequestId,
  }: {
    audioSessions: MeetingAudioSession[];
    seekTarget: AudioSeekTarget | null;
    seekRequestId: number;
  } = $props();

  let audioEl: HTMLAudioElement | undefined = $state();
  let currentPath = $state<string | null>(null);
  let playing = $state(false);
  let currentTime = $state(0);
  let duration = $state(0);
  // Guards re-applying the same click twice (e.g. an unrelated prop update
  // re-running the effect) without needing seekRequestId in a closure ref.
  let lastAppliedSeekId = -1;
  let pendingSeek: PendingSeek | null = null;
  let seekAbortController = new AbortController();

  function armMetadataListener(el: HTMLAudioElement) {
    seekAbortController.abort();
    seekAbortController = new AbortController();
    el.addEventListener(
      "loadedmetadata",
      () => {
        pendingSeek = applyPendingSeek(el, pendingSeek);
      },
      { once: true, signal: seekAbortController.signal },
    );
  }

  // Show something to play as soon as sessions arrive, before any paragraph
  // has been clicked.
  $effect(() => {
    if (currentPath === null) {
      const target = defaultAudioTarget(audioSessions);
      if (target) currentPath = target.path;
    }
  });

  $effect(() => {
    if (!seekTarget || !audioEl || seekRequestId === lastAppliedSeekId) return;
    lastAppliedSeekId = seekRequestId;

    const command = buildPlayCommand(seekTarget, currentPath);
    pendingSeek = { seekSeconds: command.seekSeconds };

    if (command.sessionChanged) {
      currentPath = command.path;
    }

    if (seekNeedsMetadata(audioEl.readyState, command.sessionChanged)) {
      armMetadataListener(audioEl);
      return;
    }

    seekAbortController.abort();
    seekAbortController = new AbortController();
    pendingSeek = applyPendingSeek(audioEl, pendingSeek);
  });

  function togglePlay() {
    if (!audioEl) return;
    if (audioEl.paused) void audioEl.play();
    else audioEl.pause();
  }

  function onScrub(event: Event) {
    if (!audioEl) return;
    audioEl.currentTime = Number((event.currentTarget as HTMLInputElement).value);
  }
</script>

{#if currentPath}
  <section class="surface-card flex items-center gap-3 px-4 py-2.5">
    <!-- svelte-ignore a11y_media_has_caption -->
    <audio
      bind:this={audioEl}
      src={convertFileSrc(currentPath)}
      preload="metadata"
      onplay={() => { playing = true; }}
      onpause={() => { playing = false; }}
      ontimeupdate={() => { if (audioEl) currentTime = audioEl.currentTime; }}
      onloadedmetadata={() => {
        if (!audioEl) return;
        duration = audioEl.duration;
        pendingSeek = applyPendingSeek(audioEl, pendingSeek);
      }}
    ></audio>
    <button
      class="btn btn-icon shrink-0"
      onclick={togglePlay}
      aria-label={playing ? $t("meeting_audio.pause") : $t("meeting_audio.play")}
    >
      {#if playing}
        <Pause size={16} />
      {:else}
        <Play size={16} />
      {/if}
    </button>
    <span class="w-10 shrink-0 text-right font-mono text-[11px] text-text-muted">{formatTimestamp(currentTime)}</span>
    <input
      type="range"
      class="flex-1"
      min="0"
      max={duration || 0}
      value={currentTime}
      oninput={onScrub}
      aria-label={$t("meeting_audio.scrubber")}
    />
    <span class="w-10 shrink-0 font-mono text-[11px] text-text-muted">{formatTimestamp(duration)}</span>
  </section>
{/if}
