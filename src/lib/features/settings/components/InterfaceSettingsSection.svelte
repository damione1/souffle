<script lang="ts">
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { Theme } from "../../../types";

  const themeOptions: Theme[] = ["dark", "light", "system"];

  let {
    theme,
    autoPaste,
    pasteDelayMs,
    toggleShortcut,
    pttShortcut,
    recordingField,
    shortcutError,
    onThemeChange,
    onAutoPasteChange,
    onPasteDelayChange,
    onStartRecording,
    onClearShortcut,
    formatShortcut,
  }: {
    theme: Theme;
    autoPaste: boolean;
    pasteDelayMs: number;
    toggleShortcut: string;
    pttShortcut: string;
    recordingField: "toggle" | "ptt" | null;
    shortcutError: string;
    onThemeChange: (theme: Theme) => void;
    onAutoPasteChange: (event: Event) => void;
    onPasteDelayChange: (event: Event) => void;
    onStartRecording: (field: "toggle" | "ptt") => void;
    onClearShortcut: (field: "toggle" | "ptt") => void | Promise<void>;
    formatShortcut: (shortcut: string) => string;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>Interface</h3>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Theme</span>
    </div>
    <div class="flex gap-1">
      {#each themeOptions as themeOption}
        <button
          onclick={() => onThemeChange(themeOption)}
          class={`btn ${theme === themeOption ? "btn-active" : ""}`}
        >
          {themeOption.charAt(0).toUpperCase() + themeOption.slice(1)}
        </button>
      {/each}
    </div>
  </div>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Auto-paste after dictation</span>
      <span class="text-sm text-text-muted">Automatically pastes the transcript when you stop recording.</span>
    </div>
    <input
      type="checkbox"
      checked={autoPaste}
      onchange={onAutoPasteChange}
      class="switch"
      aria-label="Auto-paste after dictation"
    />
  </div>

  {#if autoPaste}
    <div class="flex items-center justify-between gap-4">
      <div>
        <label for="paste-delay" class="block text-[0.9375rem] font-medium text-text-primary">Paste delay (ms)</label>
        <span class="text-sm text-text-muted">Wait time before pasting. Increase if the text lands in the wrong app.</span>
      </div>
      <input
        id="paste-delay"
        type="number"
        value={pasteDelayMs}
        onchange={onPasteDelayChange}
        min="50"
        max="1000"
        step="50"
        class="field-number"
      />
    </div>
  {/if}

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Toggle recording</span>
      <span class="text-sm text-text-muted">Press once to start or stop dictation.</span>
    </div>
    <div class="flex gap-2 items-center">
      <button
        onclick={() => onStartRecording("toggle")}
        class="shortcut-button"
        class:is-recording={recordingField === "toggle"}
      >
        {recordingField === "toggle" ? "Press keys..." : formatShortcut(toggleShortcut)}
      </button>
      {#if toggleShortcut}
        <button onclick={() => onClearShortcut("toggle")} class="btn btn-ghost text-sm">Clear</button>
      {/if}
    </div>
  </div>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Push-to-talk</span>
      <span class="text-sm text-text-muted">Hold to record, release to stop.</span>
    </div>
    <div class="flex gap-2 items-center">
      <button
        onclick={() => onStartRecording("ptt")}
        class="shortcut-button"
        class:is-recording={recordingField === "ptt"}
      >
        {recordingField === "ptt" ? "Press keys..." : formatShortcut(pttShortcut)}
      </button>
      {#if pttShortcut}
        <button onclick={() => onClearShortcut("ptt")} class="btn btn-ghost text-sm">Clear</button>
      {/if}
    </div>
  </div>

  {#if shortcutError}
    <StatusBanner message={shortcutError} variant="danger" />
  {/if}
</section>
