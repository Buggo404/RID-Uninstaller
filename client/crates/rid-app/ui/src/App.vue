<script setup lang="ts">
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { save } from "@tauri-apps/plugin-dialog";
import { computed, onMounted, onUnmounted, ref, watch, watchEffect } from "vue";
import { actionItems, jump, report } from "./actions";
import AiDialog from "./AiDialog.vue";
import { type LaunchRequest, api, formatSize, targetText } from "./api";
import HistoryDialog from "./HistoryDialog.vue";
import { lang, t } from "./i18n";
import MenuList, { type MenuItem } from "./MenuList.vue";
import { prefs, resetColumns } from "./prefs";
import PrefsDialog from "./PrefsDialog.vue";
import PromptDialog from "./PromptDialog.vue";
import RSelect from "./RSelect.vue";
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
  refreshUpdates,
  rows,
  selected,
  sizing,
  start,
  startup,
  totalKb,
  tracedView,
  tracing,
  upgrades,
  view,
} from "./store";
import StartupView from "./StartupView.vue";
import TraceDialog from "./TraceDialog.vue";
import UpdatesView from "./UpdatesView.vue";
import { asking, confirmBox, menu, notify, prompt, showMenu } from "./ui";
import logo from "../../icons/128x128.png";
import UninstallerView from "./UninstallerView.vue";
import Wizard from "./Wizard.vue";

const win = getCurrentWindow();
const maximized = ref(false);
const caption = {
  minimize: "M1 5.5h8",
  maximize: "M3 1.5h4A1.5 1.5 0 0 1 8.5 3v4A1.5 1.5 0 0 1 7 8.5H3A1.5 1.5 0 0 1 1.5 7V3A1.5 1.5 0 0 1 3 1.5z",
  restore: "M2.5 3.5h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1z M3.5 1.5h3.5A1.5 1.5 0 0 1 8.5 3v3.5",
  close: "M1.5 1.5l7 7M8.5 1.5l-7 7",
};

/** The browser's own menu never shows. Text fields and selected text get the editing commands; lists bring their own. */
function onContext(e: MouseEvent) {
  e.preventDefault();
  const field = (e.target as Element).closest<HTMLInputElement>("input:not([type=checkbox], [type=radio]), textarea");
  const picked = getSelection()?.toString() ?? "";
  if (field && !field.disabled) {
    field.focus();
    const some = field.selectionStart !== field.selectionEnd;
    const paste = (text: string) => {
      field.focus();
      field.setRangeText(text, field.selectionStart ?? 0, field.selectionEnd ?? 0, "end");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    };
    showMenu(e, [
      { label: t("edit.cut"), key: "Ctrl+X", disabled: !some || field.readOnly, run: () => document.execCommand("cut") },
      { label: t("edit.copy"), key: "Ctrl+C", disabled: !some, run: () => document.execCommand("copy") },
      { label: t("edit.paste"), key: "Ctrl+V", disabled: field.readOnly, run: () => api.clipboardText().then(paste, report) },
      { sep: true },
      { label: t("edit.selectAll"), key: "Ctrl+A", disabled: !field.value, run: () => field.select() },
    ]);
  } else if (picked.trim()) {
    showMenu(e, [{ label: t("edit.copy"), key: "Ctrl+C", run: () => navigator.clipboard.writeText(picked).catch(report) }]);
  } else menu.value = null;
}
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
  packages: "M4 8l8-4 8 4-8 4z M4 8v8l8 4 8-4V8 M12 12v8",
  recent: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M12 7v5l3 2",
  trace: "M12 3v10 M8 9l4 4 4-4 M4 17v3h16v-3",
  updates: "M20 12a8 8 0 1 1-2.3-5.7 M20 4v5h-5",
  sidebar: "M4 5h16v14H4z M9.5 5v14",
  details: "M4 5h16v14H4z M14.5 5v14",
  quarantine: "M4 8h16v11H4z M3 4h18v4H3z M10 12h4",
  bug: "M9 7a3 3 0 0 1 6 0 M7 10h10v5a5 5 0 0 1-10 0z M12 10v10 M3 13h4 M17 13h4 M4.5 7.5L7 10 M19.5 7.5L17 10 M4.5 20L7.5 17.5 M19.5 20l-3-2.5",
  github:
    "M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.1.79-.25.79-.56v-2.17c-3.2.7-3.87-1.36-3.87-1.36-.52-1.33-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.75.4-1.25.73-1.54-2.55-.29-5.24-1.28-5.24-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.52-1.46.11-3.05 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.78 0c2.2-1.49 3.17-1.18 3.17-1.18.63 1.59.23 2.76.11 3.05.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.38-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.67.8.56A11.51 11.51 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5z",
};

const categories: { id: ListCategory; glyph: keyof typeof glyphs }[] = [
  { id: "apps", glyph: "apps" },
  { id: "system", glyph: "system" },
  { id: "hidden", glyph: "hidden" },
  { id: "store", glyph: "store" },
  { id: "packages", glyph: "packages" },
];

const systemTools = ["programs", "features", "restore", "security", "sysprops", "sysinfo", "defrag", "services", "tasks"] as const;

const menus = computed<{ label: string; items: MenuItem[] }[]>(() => [
  {
    label: t("menu.file"),
    items: [
      { label: t("menu.export"), key: "Ctrl+S", run: exportHtml },
      { sep: true },
      { label: t("menu.exit"), run: () => win.close() },
    ],
  },
  { label: t("menu.action"), items: view.value === "uninstaller" ? actionItems.value : [] },
  {
    label: t("menu.view"),
    items: [
      {
        label: t("view.columns"),
        children: (["size", "date", "publisher", "version", "update", "traced"] as const).map((c) => ({
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
  {
    label: t("menu.help"),
    items: [
      { label: t("help.update"), run: () => checkUpdate(true) },
      { label: t("help.about"), key: "F1", run: () => about.value?.showModal() },
    ],
  },
]);

const version = ref("");
const update = ref<Awaited<ReturnType<typeof api.checkUpdate>>>(null);

const updating = ref(false);

async function installUpdate() {
  if (updating.value) return;
  updating.value = true;
  await api.installUpdate().catch(report);
  updating.value = false;
}

async function openUpdate() {
  const found = update.value;
  if (found && (await confirmBox(t("upd.found", found.version, found.notes).replace(/\n{3,}/, "\n\n")))) installUpdate();
}

/** `manual`: the user asked, so every outcome is reported; the check at start stays silent unless there is news. */
async function checkUpdate(manual: boolean) {
  try {
    update.value = await api.checkUpdate();
  } catch (e) {
    if (manual) report(e);
    return;
  }
  if (!manual) return void (update.value && prefs.autoUpdate && installUpdate());
  if (update.value) openUpdate();
  else notify(t("upd.latest", version.value));
}

const counts = computed(() => Object.fromEntries(categories.map((c) => [c.id, inv.value.apps.filter((a) => a.category === c.id).length])));
const moreItems = computed<MenuItem[]>(() => menus.value.map((m) => ({ label: m.label, children: m.items, disabled: !m.items.length })));
const title = computed(() => {
  if (view.value === "startup") return t("tb.startup");
  if (view.value === "updates") return t("nav.updates");
  return category.value === "recent" ? t("cat.recent", recentCount.value) : t(`cat.${category.value}`);
});

function show(c: ListCategory) {
  view.value = "uninstaller";
  category.value = c;
}

const refreshing = ref(false);
async function reload() {
  if (refreshing.value) return;
  refreshing.value = true;
  // The icon makes at least one full turn, so even an instant refresh is seen to have happened.
  const turn = new Promise((done) => setTimeout(done, 600));
  try {
    if (view.value === "updates") await refreshUpdates();
    else await (view.value === "startup" ? refreshStartup() : refresh());
  } catch (e) {
    report(e);
  } finally {
    await turn;
    refreshing.value = false;
  }
}
/** Labels are hidden while the sidebar is collapsed; they are offered as tooltips instead. */
const tip = (label: string) => (prefs.sidebar ? undefined : label);
const recentLabel = computed(() => t("cat.recent", "").replace(/\s*[(（].*$/, ""));
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
  localStorage.setItem("rid.theme", root.dataset.theme);
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
  if (ctrl && e.key.toLowerCase() === "b") return act(() => (prefs.sidebar = !prefs.sidebar));
  if (e.key === "Escape") {
    if (menu.value) return (menu.value = null);
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
function handleLaunch(request: LaunchRequest | null) {
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
  addEventListener("contextmenu", onContext);
  addEventListener("blur", () => (menu.value = null));
  refresh().then(api.launchRequest).then(handleLaunch).catch(report);
  // Rid started again while this window is open: the request comes here instead.
  void listen<LaunchRequest>("launch", (e) => handleLaunch(e.payload));
  const sized = () => win.isMaximized().then((is) => (maximized.value = is), console.error);
  void win.onResized(sized);
  sized();
  api.appVersion().then((v) => (version.value = v), console.error);
  if (prefs.checkUpdates) checkUpdate(false);
});
onUnmounted(() => removeEventListener("keydown", onKey));
</script>

<template>
  <div class="app" @click="(openMenu = false), (menu = null)">
    <nav class="sidebar" :class="{ collapsed: !prefs.sidebar }">
      <div class="brand" data-tauri-drag-region>
        <!-- The mark opens the menu too: it is all that is left of this row when the sidebar is collapsed. -->
        <button type="button" class="brand-mark" aria-haspopup="menu" :aria-expanded="openMenu" :aria-label="t('nav.menu')" :title="t('nav.menu')" @click.stop="openMenu = !openMenu">
          <img :src="logo" alt="" />
        </button>
        <span class="brand-name" data-tauri-drag-region>RID</span>
        <button type="button" class="icon-btn" tabindex="-1" aria-hidden="true" @click.stop="openMenu = !openMenu">
          <svg viewBox="0 0 16 16"><path d="M3 8h.01M8 8h.01M13 8h.01" /></svg>
        </button>
      </div>
      <!-- Fixed to the window rather than placed inside the sidebar, which clips what leaves it. -->
      <MenuList v-if="openMenu" class="context app-menu" :items="moreItems" @close="openMenu = false" />

      <div class="nav-label">{{ t("nav.programs") }}</div>
      <button
        v-for="c in categories"
        v-show="c.id !== 'packages' || counts.packages"
        :key="c.id"
        type="button"
        class="nav"
        :class="{ active: view === 'uninstaller' && category === c.id }"
        :aria-current="view === 'uninstaller' && category === c.id ? 'page' : undefined"
        :title="tip(t(`cat.${c.id}`))"
        @click="show(c.id)"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs[c.glyph]" /></svg>
        <span class="nav-text">{{ t(`cat.${c.id}`) }}</span>
        <span class="count">{{ counts[c.id] || "" }}</span>
      </button>
      <button
        v-if="prefs.recentInToolbar && recentCount"
        type="button"
        class="nav"
        :class="{ active: view === 'uninstaller' && category === 'recent' }"
        :title="tip(recentLabel)"
        @click="show('recent')"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.recent" /></svg>
        <span class="nav-text">{{ recentLabel }}</span>
        <span class="count">{{ recentCount }}</span>
      </button>

      <div class="nav-label">{{ t("nav.tools") }}</div>
      <button
        type="button"
        class="nav"
        :class="{ active: view === 'updates' }"
        :title="tip(t('nav.updates'))"
        @click="view = 'updates'"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.updates" /></svg>
        <span class="nav-text">{{ t("nav.updates") }}</span>
        <span class="count">{{ upgrades.items.length || "" }}</span>
      </button>
      <button type="button" class="nav" :class="{ active: view === 'startup' }" :title="tip(t('tb.startup'))" @click="view = 'startup'">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.startup" /></svg>
        <span class="nav-text">{{ t("tb.startup") }}</span>
      </button>
      <button type="button" class="nav" :title="tip(t('cat.trace'))" @click="tracing = {}">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.trace" /></svg>
        <span class="nav-text">{{ t("cat.trace") }}</span>
      </button>
      <button type="button" class="nav" :title="tip(t('hist.title'))" @click="showHistory = true">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.quarantine" /></svg>
        <span class="nav-text">{{ t("hist.title") }}</span>
      </button>

      <span class="spacer"></span>
      <button v-if="update" type="button" class="nav" :disabled="updating" :title="tip(t('upd.nav', update.version))" @click="openUpdate">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.trace" /></svg>
        <span class="nav-text">{{ updating ? t("upd.busy") : t("upd.nav", update.version) }}</span>
        <span class="count"><i class="status-dot running"></i></span>
      </button>
      <div class="nav-pair">
        <button type="button" class="nav" :title="tip('GitHub')" @click="api.openUrl(api.repoUrl).catch(report)">
          <svg class="solid" viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.github" /></svg>
          <span class="nav-text">GitHub</span>
        </button>
        <button type="button" class="nav" :title="tip(t('nav.bug'))" @click="api.openUrl(`${api.repoUrl}/issues`).catch(report)">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.bug" /></svg>
          <span class="nav-text">{{ t("nav.bug") }}</span>
        </button>
      </div>
      <button type="button" class="nav" :title="tip(t('tb.prefs'))" @click="showPrefs = true">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.prefs" /></svg>
        <span class="nav-text">{{ t("tb.prefs") }}</span>
        <kbd class="count">F9</kbd>
      </button>
    </nav>

    <div class="main">
      <header class="header" data-tauri-drag-region>
        <button
          type="button"
          class="icon-btn"
          :aria-expanded="prefs.sidebar"
          :aria-label="t(prefs.sidebar ? 'nav.collapse' : 'nav.expand')"
          :title="`${t(prefs.sidebar ? 'nav.collapse' : 'nav.expand')} (Ctrl+B)`"
          @click="prefs.sidebar = !prefs.sidebar"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.sidebar" /></svg>
        </button>
        <h1 data-tauri-drag-region>{{ title }}</h1>
        <RSelect v-if="view === 'startup'" v-model="startup.group" class="header-select" :options="startupGroups.map((g) => ({ value: g, label: t(`st.${g}`) }))" />
        <span class="spacer drag" data-tauri-drag-region></span>
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
        <button v-if="view === 'startup'" type="button" class="icon-btn" :title="t('st.add')" :aria-label="t('st.add')" @click="startup.adding = true">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
        </button>
        <button
          type="button"
          class="icon-btn"
          :class="{ spinning: refreshing }"
          :disabled="refreshing"
          :title="`${t('tb.refresh')} (F5)`"
          :aria-label="t('tb.refresh')"
          @click="reload"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.refresh" /></svg>
        </button>
        <button
          type="button"
          class="icon-btn"
          :aria-expanded="prefs.details"
          :aria-label="t(prefs.details ? 'panel.hide' : 'panel.show')"
          :title="t(prefs.details ? 'panel.hide' : 'panel.show')"
          @click="prefs.details = !prefs.details"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="glyphs.details" /></svg>
        </button>
        <div class="window-buttons">
          <button type="button" :aria-label="t('win.minimize')" @click="win.minimize()">
            <svg viewBox="0 0 10 10" aria-hidden="true"><path :d="caption.minimize" /></svg>
          </button>
          <button type="button" :aria-label="t(maximized ? 'win.restore' : 'win.maximize')" @click="win.toggleMaximize()">
            <svg viewBox="0 0 10 10" aria-hidden="true"><path :d="maximized ? caption.restore : caption.maximize" /></svg>
          </button>
          <button type="button" class="close" :aria-label="t('win.close')" @click="win.close()">
            <svg viewBox="0 0 10 10" aria-hidden="true"><path :d="caption.close" /></svg>
          </button>
        </div>
      </header>

      <UninstallerView v-if="view === 'uninstaller'" ref="uninstaller" />
      <StartupView v-else-if="view === 'startup'" />
      <UpdatesView v-else />

      <footer class="status" role="status">
        <template v-if="view === 'uninstaller'">
          <span>{{ t("status.total", rows.length, formatSize(totalKb) || "0 KB") }}</span>
          <span v-if="sizing">{{ t("status.sizing") }}</span>
          <span v-if="inv.warnings.length" :title="inv.warnings.join('\n')">{{ t("status.warnings", inv.warnings.length) }}</span>
        </template>
        <template v-else-if="view === 'updates'">
          <span>{{ upgrades.loading ? t("upd.checking") : t("upd.total", upgrades.items.length) }}</span>
          <span v-if="upgrades.warnings.length" :title="upgrades.warnings.join('\n')">{{ t("status.warnings", upgrades.warnings.length) }}</span>
        </template>
        <span v-else>{{ t("st.total", startup.items.length, startup.items.filter((i) => i.status === "running").length) }}</span>
      </footer>
    </div>

    <PrefsDialog v-if="showPrefs" @close="showPrefs = false" />

    <dialog ref="about" class="dialog prompt" aria-labelledby="about-title">
      <h2 id="about-title">Rid Uninstaller {{ version }}</h2>
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

    <AiDialog v-if="asking" :key="JSON.stringify(asking.question)" v-bind="asking" @close="asking = null" />

    <!-- Last, so that it opens above whichever dialog asked. -->
    <PromptDialog v-if="prompt" :key="prompt.text" :prompt="prompt" />

    <Teleport v-if="menu" :to="menu.host">
      <MenuList :key="`${menu.x},${menu.y}`" class="context" :style="{ left: `${menu.x}px`, top: `${menu.y}px` }" :items="menu.items" @close="menu = null" />
    </Teleport>
  </div>
</template>
