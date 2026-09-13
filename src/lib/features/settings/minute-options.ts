/** One entry of a numeric settings dropdown. */
export interface MinuteOption {
  value: number;
  /** i18n key for the label, or `null` when the backend offers a value this
   * file has no wording for. */
  labelKey: string | null;
}

/** Build a numeric settings dropdown from the list the backend serves.
 *
 * The values come from `settings::SettingsOptions`, which sits next to the
 * bounds `sanitize_for_save` validates against, so the UI can no longer offer
 * a step the backend would fold back to the default. Only the wording lives
 * on this side.
 *
 * A value with no label renders as the bare number rather than disappearing.
 * That is deliberate: a silently dropped option is exactly the failure this
 * change exists to remove, and it cannot happen today because
 * `offered_options_survive_sanitize` and `offered_options_are_unchanged` pin
 * the Rust lists. */
export function minuteOptions(
  values: readonly number[] | undefined,
  labelKeys: Readonly<Record<number, string>>,
): MinuteOption[] {
  return (values ?? []).map((value) => ({
    value,
    labelKey: labelKeys[value] ?? null,
  }));
}
