import { createApp } from "vue";
import App from "./App.vue";
import { loadPrefs } from "./prefs";
import "./style.css";
import "./controls.css";

loadPrefs().then(() => createApp(App).mount("#app"));
