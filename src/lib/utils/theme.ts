import type { Theme } from "../types";

const THEME_STORAGE_KEY = "souffle-theme";

/** Apply theme class to document root element and persist the setting for
 * the pre-paint script in index.html, which mirrors this resolution. */
export function applyTheme(theme: Theme): void {
  const isDark =
    theme === "dark" ||
    (theme === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", isDark);
  document.documentElement.classList.toggle("light", !isDark);

  try {
    localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // localStorage may be blocked; silently continue
  }
}
