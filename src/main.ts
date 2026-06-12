import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { mount } from "svelte";
import "./app.css";
import { initI18n } from "./lib/i18n";
import App from "./App.svelte";
import PillApp from "./lib/pill/PillApp.svelte";

initI18n();

// The same SPA serves both windows; the floating recording pill gets its
// own minimal root instead of the full app shell.
const isPill = getCurrentWebviewWindow().label === "pill";

const app = mount(isPill ? PillApp : App, {
  target: document.getElementById("app")!,
});

export default app;
