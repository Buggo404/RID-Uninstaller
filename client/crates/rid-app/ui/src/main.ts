import { createApp } from "vue";
import App from "./App.vue";
import { loadPrefs } from "./prefs";
import "./style.css";
import "./controls.css";

// The theme of the last run, applied before anything else so the first frame is already right.
document.documentElement.dataset.theme =
  localStorage.getItem("rid.theme") ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

loadPrefs().then(() => createApp(App).mount("#app"));
