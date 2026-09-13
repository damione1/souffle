import { mount } from "svelte";
import "./app.css";
import { initI18n } from "./lib/i18n";
import { primeSettingsDefaults } from "./lib/bootstrap";
import { getAppState } from "./lib/stores/app.svelte";
import App from "./App.svelte";

initI18n();

// The store carries no hand-written defaults: `AppSettings::default()` in
// src-tauri/src/settings.rs is the only declaration, and it reaches the webview
// through `get_default_settings`, which reads no database and therefore answers
// on a first launch. Mounting before it lands would let a component read
// settings that do not exist yet, so the first frame waits for it.
// `bootstrapAppState` reads the stored settings right after mount and
// overwrites these.
await primeSettingsDefaults(getAppState());

const app = mount(App, {
  target: document.getElementById("app")!,
});

export default app;
