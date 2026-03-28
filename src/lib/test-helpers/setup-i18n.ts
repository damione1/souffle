import { addMessages, init } from "svelte-i18n";
import en from "../i18n/en.json";

addMessages("en", en);
init({ fallbackLocale: "en", initialLocale: "en" });
