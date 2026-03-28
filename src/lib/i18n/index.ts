import { addMessages, init, getLocaleFromNavigator, locale } from "svelte-i18n";
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

export { locale } from "svelte-i18n";
