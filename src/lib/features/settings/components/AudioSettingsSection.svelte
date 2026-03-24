<script lang="ts">
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
  <h3>Audio Configuration</h3>
  <p class="text-text-secondary text-sm">Choose the active microphone or virtual device.</p>

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
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16">
          <path fill-rule="evenodd" d="M15.312 11.424a5.5 5.5 0 0 1-9.201 2.466l-.312-.311h2.433a.75.75 0 0 0 0-1.5H4.598a.75.75 0 0 0-.75.75v3.634a.75.75 0 0 0 1.5 0v-2.033l.312.311a7 7 0 0 0 11.712-3.138.75.75 0 0 0-1.449-.389Zm-11.23-3.27a.75.75 0 0 0 1.449.39A5.5 5.5 0 0 1 14.7 6.079l.312.311H12.78a.75.75 0 0 0 0 1.5h3.634a.75.75 0 0 0 .75-.75V3.506a.75.75 0 0 0-1.5 0v2.033l-.312-.311A7 7 0 0 0 3.693 8.343Z" clip-rule="evenodd" />
        </svg>
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
