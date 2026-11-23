import { createApp } from "vue";
import App from "./App.vue";
import { loadPrefs } from "./prefs";
import "./style.css";
import "./controls.css";

// The theme of the last run, applied before anything else so the first frame is already right.
document.documentElement.dataset.theme =
  localStorage.getItem("rid.theme") ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

// A dialog's `close` event is what makes its owner take it off the page, which would cut the
// closing animation short. The event is held back here until the dialog has faded (0.16s in controls.css).
const faded = new WeakSet<Event>();
document.addEventListener(
  "close",
  (e) => {
    if (faded.has(e) || !(e.target instanceof HTMLDialogElement)) return;
    e.stopImmediatePropagation();
    const later = new Event("close");
    faded.add(later);
    const target = e.target;
    setTimeout(() => target.dispatchEvent(later), matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 160);
  },
  true,
);

loadPrefs().then(() => createApp(App).mount("#app"));
