<script lang="ts">
  import { RefreshCw } from "@lucide/svelte";
  import type { AudioDeviceInfo } from "../../../types";

  let {
    audioDevices,
    selectedDevice,
    onDeviceChange,
    onRefreshDevices,
  }: {
    audioDevices: AudioDeviceInfo[];
    selectedDevice: string;
    onDeviceChange: (event: Event) => void | Promise<void>;
    onRefreshDevices: () => void | Promise<void>;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>Audio</h3>
  <p class="text-text-secondary text-sm">Choose which microphone Souffle listens to.</p>

  <div class="flex flex-col gap-1.5">
    <label for="input-device" class="field-label">Input device</label>
    <div class="flex gap-1.5 items-center">
      <select id="input-device" value={selectedDevice} onchange={onDeviceChange} class="field-select">
        {#each audioDevices as device}
          <option value={device.name}>
            {device.name}{device.is_default ? " (default)" : ""}
          </option>
        {/each}
      </select>
      <button onclick={onRefreshDevices} class="btn btn-icon" aria-label="Refresh devices">
        <RefreshCw size={16} />
      </button>
    </div>
  </div>

  <div class="flex items-center justify-between gap-4 opacity-50">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Noise Reduction</span>
      <span class="text-sm text-text-muted">Reduce background noise during capture.</span>
    </div>
    <div class="flex gap-2 items-center">
      <span class="pill pill-muted">Coming Soon</span>
      <input type="checkbox" disabled class="switch" />
    </div>
  </div>
</section>
