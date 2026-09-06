import type { AudioInputDevice } from "../../types";

/** Teams/Zoom fail to start on a shared mic above this rate. */
export const CONFERENCING_SAMPLE_RATE_HZ = 48_000;

export function sampleRateBlocksConferencing(hz: number | null | undefined): boolean {
  return hz != null && hz > CONFERENCING_SAMPLE_RATE_HZ;
}

export function formatSampleRateHz(hz: number): string {
  if (hz % 1000 === 0) return `${hz / 1000} kHz`;
  if (hz % 100 === 0) return `${(hz / 1000).toFixed(1)} kHz`;
  return `${hz} Hz`;
}

/** UID whose rate Settings should show: the pin if connected, else the default input. */
export function resolveSampleRateDeviceUid(
  selectedDevice: string,
  devices: AudioInputDevice[],
): string | null {
  if (selectedDevice && devices.some((device) => device.uid === selectedDevice)) {
    return selectedDevice;
  }
  return devices.find((device) => device.is_default)?.uid ?? devices[0]?.uid ?? null;
}
