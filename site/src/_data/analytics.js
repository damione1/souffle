// SPDX-License-Identifier: GPL-3.0-or-later
// Analytics is configured by the build environment; local builds can omit it.
export default function () {
  const measurementId = process.env.GA_MEASUREMENT_ID || '';
  if (measurementId && !/^G-[A-Z0-9]+$/.test(measurementId)) {
    throw new Error('GA_MEASUREMENT_ID must be a valid GA4 measurement ID');
  }
  return {measurementId};
}
