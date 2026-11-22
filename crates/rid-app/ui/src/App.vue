<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { save } from "@tauri-apps/plugin-dialog";
import { computed, onMounted, onUnmounted, ref, watch, watchEffect } from "vue";
import { actionItems, jump, report } from "./actions";
import { api, formatSize, targetText } from "./api";
import HistoryDialog from "./HistoryDialog.vue";
import { lang, t } from "./i18n";
import MenuList, { type MenuItem } from "./MenuList.vue";
import { prefs, resetColumns } from "./prefs";
import PrefsDialog from "./PrefsDialog.vue";
import PromptDialog from "./PromptDialog.vue";
import {
  type ListCategory,
  type StartupGroup,
  category,
  filter,
  inv,
  job,
  recentCount,
  refresh,
  refreshStartup,
  rows,
  selected,
  sizing,
  start,
  startup,
  totalKb,
  tracedView,
  tracing,
  view,
} from "./store";
import StartupView from "./StartupView.vue";
import TraceDialog from "./TraceDialog.vue";
import { prompt } from "./ui";
import UninstallerView from "./UninstallerView.vue";
import Wizard from "./Wizard.vue";

const openMenu = ref(false);
const searchInput = ref<HTMLInputElement>();
const showPrefs = ref(false);
const showHistory = ref(false);
const about = ref<HTMLDialogElement>();
const tracedDialog = ref<HTMLDialogElement>();
// The element exists only while there is a record to show; open it as a modal once it does.
watch(tracedDialog, (el) => el?.showModal());
const uninstaller = ref<InstanceType<typeof UninstallerView>>();

const glyphs = {
  uninstaller: "M4 7l8-4 8 4v10l-8 4-8-4z M4 7l8 4 8-4 M12 11v10",
  startup: "M12 3v9 M6.3 6.3a8 8 0 1 0 11.4 0",
  refresh: "M20 12a8 8 0 1 1-2.3-5.7 M20 4v3h-3",
  prefs: "M4 7h10 M18 7h2 M4 12h4 M12 12h8 M4 17h12 M20 17h0 M16 5v4 M10 10v4 M18 15v4",
  help: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 .9-1 1.7 M12 17h.01",
  apps: "M4 4h7v7H4z M13 4h7v7h-7z M4 13h7v7H4z M13 13h7v7h-7z",
  system: "M3 5h18v11H3z M8 20h8 M12 16v4",
  hidden: "M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6z M4 4l16 16",
  store: "M5 8h14l-1 12H6z M9 8V6a3 3 0 0 1 6 0v2",
  recent: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M12 7v5l3 2",
  trace: "M12 3v10 M8 9l4 4 4-4 M4 17v3h16v-3",
};

const categories: { id: ListCategory; glyph: keyof typeof glyphs }[] = [
  { id: "apps", glyph: "apps" },
  { id: "system", glyph: "system" },
  { id: "hidden", glyph: "hidden" },
  { id: "store", glyph: "store" },
];

const systemTools = ["programs", "features", "restore", "security", "sysprops", "sysinfo", "defrag", "services", "tasks"] as const;

const menus = computed<{ label: string; items: MenuItem[] }[]>(() => [
  {
    label: t("menu.file"),
    items: [
      { label: t("menu.export"), key: "Ctrl+S", run: exportHtml },
      { sep: true },
      { label: t("menu.exit"), run: () => getCurrentWindow().close() },
    ],
  },
  { label: t("menu.action"), items: view.value === "uninstaller" ? actionItems.value : [] },
  {
    label: t("menu.view"),
    items: [
      {
        label: t("view.columns"),
        children: (["size", "date", "publisher", "version", "traced"] as const).map((c) => ({
          label: t(`col.${c}`),
          checked: prefs.columns[c],
          run: () => (prefs.columns[c] = !prefs.columns[c]),
        })),
      },
      { label: t("view.resetColumns"), run: resetColumns },
      { label: t("view.refresh"), key: "F5", run: reload },
      { sep: true },
      {
        label: t("view.language"),
        children: [
          { label: t("view.langAuto"), checked: prefs.lang === "auto", run: () => (prefs.lang = "auto") },
          { label: "简体中文", checked: prefs.lang === "zh", run: () => (prefs.lang = "zh") },
          { label: "English", checked: prefs.lang === "en", run: () => (prefs.lang = "en") },
        ],
      },
    ],
  },
  {
    label: t("menu.tools"),
    items: [
      {
        label: t("tools.system"),
        children: systemTools.map((name) => ({
          label: t(`tools.${name}`),
          run: () => api.systemTool(name).catch(report),
        })),
      },
      { sep: true },
      { label: t("tools.history"), run: () => (showHistory.value = true) },
      { label: t("tools.prefs"), key: "F9", run: () => (showPrefs.value = true) },
    ],
  },
  { label: t("menu.help"), items: [{ label: t("help.about"), key: "F1", run: () => about.value?.showModal() }] },
]);

const counts = computed(() => Object.fromEntries(categories.map((c) => [c.id, inv.value.apps.filter((a) => a.category === c.id).length])));
const moreItems = computed<MenuItem[]>(() => menus.value.map((m) => ({ label: m.label, children: m.items, disabled: !m.items.length })));
const title = computed(() => {
  if (view.value === "startup") return t("tb.startup");
  return category.value === "recent" ? t("cat.recent", recentCount.value) : t(`cat.${category.value}`);
});

function show(c: ListCategory) {
  view.value = "uninstaller";
  category.value = c;
}

const reload = () => (view.value === "startup" ? refreshStartup() : refresh()).catch(report);
const startupGroups: StartupGroup[] = ["all", "registry", "folder", "tasks", "store"];
watch(view, () => (filter.value = ""));

function finished(changed: boolean) {
  job.value = null;
  if (changed) reload();
}

async function exportHtml() {
  const path = await save({ defaultPath: "programs.html", filters: [{ name: t("export.filter"), extensions: ["html"] }] });
  if (!path) return;
  const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
  const cells = (values: string[], tag: string) => `<tr>${values.map((v) => `<${tag}>${esc(v)}</${tag}>`).join("")}</tr>`;
  const head = cells([t("col.name"), t("col.version"), t("col.publisher"), t("col.size"), t("col.date")], "th");
  const body = rows.value
    .map(({ app }) => cells([app.name, app.version, app.publisher, formatSize(app.size_kb), app.install_date ?? ""], "td"))
    .join("\n");
  const title = t("export.title", rows.value.length);
  const html = `<!doctype html>
<html lang="${lang.value === "zh" ? "zh-CN" : "en"}"><head><meta charset="utf-8"><title>${esc(title)}</title>
<style>body{font:14px "Segoe UI",sans-serif;margin:24px}table{border-collapse:collapse}th,td{padding:4px 12px;border:1px solid #ccc;text-align:left}</style>
</head><body><h1>${esc(title)}</h1>
<table>
${head}
${body}
</table>
<p>${esc(t("export.generated", "Rid Uninstaller", new Date().toLocaleString()))}</p>
</body></html>
`;
  api.saveTextFile(path, html).catch(report);
}

const systemDark = matchMedia("(prefers-color-scheme: dark)");
const dark = ref(systemDark.matches);
systemDark.addEventListener("change", (e) => (dark.value = e.matches));
watchEffect(() => {
  const root = document.documentElement;
  root.dataset.theme = prefs.theme === "auto" ? (dark.value ? "dark" : "light") : prefs.theme;
  root.lang = lang.value === "zh" ? "zh-CN" : "en";
});

function onKey(e: KeyboardEvent) {
  if (document.querySelector("dialog[open]")) return;
  const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement;
  const ctrl = e.ctrlKey && !e.altKey && !e.shiftKey;
  const act = (run: () => unknown) => {
    e.preventDefault();
    run();
  };
  if (e.key === "F5") return act(reload);
  if (e.key === "F9") return act(() => (showPrefs.value = true));
  if (e.key === "F1") return act(() => about.value?.showModal());
  if (e.ctrlKey && e.key === "Tab") return act(() => (view.value = view.value === "uninstaller" ? "startup" : "uninstaller"));
  if (ctrl && e.key.toLowerCase() === "s") return act(exportHtml);
  if (e.key === "Escape") {
    if (openMenu.value) return (openMenu.value = false);
    if (filter.value) return act(() => ((filter.value = ""), uninstaller.value?.focusList()));
  }
  if (ctrl && view.value === "uninstaller" && e.key.toLowerCase() === "g") return act(jump.google);
  if (ctrl && view.value === "uninstaller" && e.key.toLowerCase() === "r") return act(jump.registry);
  if (ctrl && view.value === "uninstaller" && e.key.toLowerCase() === "e") return act(jump.folder);
  if (ctrl && view.value === "uninstaller" && e.key.toLowerCase() === "w") return act(jump.website);
  if (typing) return;
  // Typing anywhere starts a search; the focused input receives this same keystroke.
  if (e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.altKey && !e.metaKey) return searchInput.value?.focus();
  if (view.value !== "uninstaller") return;
  if (e.key === "Enter") return act(() => start("uninstall"));
  if (e.key === "Delete" && e.shiftKey) return act(() => start("force"));
}

/** Acts on what Explorer's context menu (or a command line) asked for, once the list is loaded. */
async function handleLaunch() {
  const request = await api.launchRequest();
  if (!request) return;
  if (request.kind === "startup") view.value = "startup";
  else if (request.kind === "store_apps") category.value = "store";
  else if (request.kind === "trace") tracing.value = { path: request.path };
  else {
    const app = inv.value.apps.find((a) => a.id === request.app_id);
    if (!app) return report(t("launch.noProgram", request.shortcut));
    category.value = app.category;
    selected.clear();
    selected.add(app.id);
    job.value = { mode: "uninstall", apps: [app], ask: true };
  }
}

onMounted(() => {
  addEventListener("keydown", onKey);
  refresh().then(handleLaunch).catch(report);
});
onUnmounted(() => removeEventListener("keydown", onKey));
</script>

<template>
  <div class="app" @click="openMenu = false">
    <nav class="sidebar">
      <div class="brand">
        <span class="brand-mark" aria-hidden="true">R</span>
        <span>Rid Uninstaller</span>
        <div class="menu-anchor">
          <button type="button" class="icon-btn" aria-haspopup="menu" :aria-expanded="openMenu" :aria-label="t('nav.menu')" @click.stop="openMenu = !openMenu">
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8h.01M8 8h.01M13 8h.01" /></svg>
          </button>
          <Transition name="pop">
            <MenuList v-if="openMenu" :items="moreItems" @close="openMenu = false" />
          </Transition>
        </div>
      </div>

      <div class="nav-label">{{ t("nav.programs") }}</div>
      <button
        v-for="c in categories"
        :key="c.id"
        type="button"
        class="nav"
        :class="{ active: view === 'uninstaller' && category === c.id }"
        :aria-current="view === 'uninstaller' && category === c.id ? 'page' : undefined"
        @click="show(c.id)"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs[c.glyph]" /></svg>
        <span>{{ t(`cat.${c.id}`) }}</span>
        <span class="count">{{ counts[c.id] || "" }}</span>
      </button>
      <button
        v-if="prefs.recentInToolbar && recentCount"
        type="button"
        class="nav"
        :class="{ active: view === 'uninstaller' && category === 'recent' }"
        @click="show('recent')"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.recent" /></svg>
        <span>{{ t("cat.recent", "").replace(/\s*[(（].*$/, "") }}</span>
        <span class="count">{{ recentCount }}</span>
      </button>

      <div class="nav-label">{{ t("nav.tools") }}</div>
      <button type="button" class="nav" :class="{ active: view === 'startup' }" @click="view = 'startup'">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.startup" /></svg>
        <span>{{ t("tb.startup") }}</span>
      </button>
      <button type="button" class="nav" @click="tracing = {}">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.trace" /></svg>
        <span>{{ t("cat.trace") }}</span>
      </button>
      <button type="button" class="nav" @click="showHistory = true">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.recent" /></svg>
        <span>{{ t("hist.title") }}</span>
      </button>

      <span class="spacer"></span>
      <button type="button" class="nav" @click="showPrefs = true">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.prefs" /></svg>
        <span>{{ t("tb.prefs") }}</span>
        <kbd class="count">F9</kbd>
      </button>
    </nav>

    <div class="main">
      <header class="header">
        <h1>{{ title }}</h1>
        <div v-if="view === 'startup'" class="seg" role="tablist">
          <button
            v-for="g in startupGroups"
            :key="g"
            type="button"
            role="tab"
            :aria-selected="startup.group === g"
            :class="{ active: startup.group === g }"
            @click="startup.group = g"
          >
            {{ t(`st.${g}`) }}
          </button>
        </div>
        <span class="spacer"></span>
        <div class="search">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M7 12A5 5 0 1 0 7 2a5 5 0 0 0 0 10zM14 14l-3.5-3.5" /></svg>
          <input
            ref="searchInput"
            v-model="filter"
            class="input"
            type="text"
            autocomplete="off"
            spellcheck="false"
            :aria-label="t('panel.search')"
            :placeholder="t('panel.searchTip')"
          />
        </div>
        <button v-if="view === 'startup'" type="button" class="btn" @click="startup.adding = true">{{ t("st.add") }}</button>
        <button type="button" class="icon-btn" :title="`${t('tb.refresh')} (F5)`" :aria-label="t('tb.refresh')" @click="reload">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.refresh" /></svg>
        </button>
      </header>

      <UninstallerView v-if="view === 'uninstaller'" ref="uninstaller" />
      <StartupView v-else />

      <footer class="status" role="status">
        <template v-if="view === 'uninstaller'">
          <span>{{ t("status.total", rows.length, formatSize(totalKb) || "0 KB") }}</span>
          <span v-if="sizing">{{ t("status.sizing") }}</span>
          <span v-if="inv.warnings.length" :title="inv.warnings.join('\n')">{{ t("status.warnings", inv.warnings.length) }}</span>
        </template>
        <span v-else>{{ t("st.total", startup.items.length, startup.items.filter((i) => i.status === "running").length) }}</span>
      </footer>
    </div>

    <PrefsDialog v-if="showPrefs" @close="showPrefs = false" />

    <dialog ref="about" class="dialog prompt" aria-labelledby="about-title">
      <h2 id="about-title">Rid Uninstaller 0.1.0</h2>
      <p class="note">{{ t("about.text") }}</p>
      <form method="dialog" class="dialog-buttons">
        <span class="spacer"></span>
        <button class="btn primary" autofocus>{{ t("btn.close") }}</button>
      </form>
    </dialog>

    <!-- Keyed so that a new job always starts a fresh wizard. -->
    <Wizard v-if="job" :key="job.apps[0].id + job.mode" :mode="job.mode" :apps="job.apps" :ask="job.ask" @close="finished" />
    <HistoryDialog v-if="showHistory" @close="(showHistory = false), finished($event)" />
    <TraceDialog v-if="tracing" :append-to="tracing.appendTo" :initial-path="tracing.path" @close="(tracing = null), finished($event)" />

    <dialog v-if="tracedView" ref="tracedDialog" class="dialog wizard" aria-labelledby="traced-title" @close="tracedView = null">
      <h2 id="traced-title">{{ t("tr.viewTitle", tracedView.name) }}</h2>
      <div class="traces plain">
        <div v-for="(item, i) in tracedView.items" :key="i" class="trace-path">{{ targetText(item) }}</div>
      </div>
      <form method="dialog" class="dialog-buttons">
        <span class="spacer"></span>
        <button class="btn primary" autofocus>{{ t("btn.close") }}</button>
      </form>
    </dialog>

    <!-- Last, so that it opens above whichever dialog asked. -->
    <PromptDialog v-if="prompt" :key="prompt.text" :prompt="prompt" />
  </div>
</template>
