import { get } from "svelte/store";
import { addMessages, init, getLocaleFromNavigator, locale, t } from "svelte-i18n";
import en from "./en.json";
import fr from "./fr.json";

export const SUPPORTED_LOCALES = [
  { id: "en", label: "English" },
  { id: "fr", label: "Français" },
] as const;

export type SupportedLocale = (typeof SUPPORTED_LOCALES)[number]["id"];

addMessages("en", en);
addMessages("fr", fr);

export function initI18n(savedLocale?: string) {
  const fallback = "en";
  const initialLocale = savedLocale || getLocaleFromNavigator()?.split("-")[0] || fallback;

  init({
    fallbackLocale: fallback,
    initialLocale: SUPPORTED_LOCALES.some((l) => l.id === initialLocale) ? initialLocale : fallback,
  });
}

export function setLocale(loc: string) {
  if (SUPPORTED_LOCALES.some((l) => l.id === loc)) {
    locale.set(loc);
  }
}

/** Translate from a `.ts` module (controllers) the same way `$t` does in Svelte. */
export function tr(key: string, values?: Record<string, string | number>): string {
  return get(t)(key, values ? { values } : undefined);
}

export { locale } from "svelte-i18n";
