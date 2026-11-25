<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { actionItems, askAbout, canUninstall, jump } from "./actions";
import { type App, type InstallerInfo, SOURCES, api, appIcon, formatSize } from "./api";
import { useColumns } from "./columns";
import { type MsgKey, lang, t } from "./i18n";
import { type SortKey, prefs } from "./prefs";
import { type Row, category, current, filter, isRecent, loading, rows, selected, selection, setOpen, sizeOf, start } from "./store";
import TableHead from "./TableHead.vue";
import { showMenu } from "./ui";

const listEl = ref<HTMLElement>();
const brokenIcons = ref(new Set<string>());
let anchor: string | null = null;

defineExpose({ focusList: () => listEl.value?.focus() });

const cols = useColumns("uninstaller", () => [
  { key: "name", label: t("col.name"), width: 0, sortable: true },
  { key: "size", label: t("col.size"), width: 92, sortable: true, hidden: !prefs.columns.size },
  { key: "date", label: t("col.date"), width: 124, sortable: true, hidden: !prefs.columns.date },
  { key: "publisher", label: t("col.publisher"), width: 190, sortable: true, hidden: !prefs.columns.publisher },
  { key: "version", label: t("col.version"), width: 130, sortable: true, hidden: !prefs.columns.version },
  { key: "update", label: t("col.update"), width: 110, hidden: !prefs.columns.update },
  { key: "traced", label: t("col.traced"), width: 64, hidden: !prefs.columns.traced },
]);
const span = computed(() => cols.columns.length + (cols.filler ? 1 : 0));

// Only the rows in view (plus a margin) are in the DOM, so a machine with thousands of
// programs scrolls like one with fifty and only asks for the icons it shows.
const HEAD = 33;
const rowH = computed(() => (prefs.compact ? 30 : 40));
const scrollTop = ref(0);
const viewH = ref(800);
const first = computed(() => Math.max(0, Math.floor(scrollTop.value / rowH.value) - 8));
const last = computed(() => Math.min(rows.value.length, Math.ceil((scrollTop.value + viewH.value) / rowH.value) + 8));
const visible = computed(() => rows.value.slice(first.value, last.value));

const resized = new ResizeObserver(([entry]) => (viewH.value = entry.contentRect.height));
onMounted(() => listEl.value && resized.observe(listEl.value));
onUnmounted(() => resized.disconnect());
watch([category, filter], () => listEl.value?.scrollTo(0, 0));

function reveal(index: number) {
  const el = listEl.value;
  if (!el) return;
  const top = index * rowH.value;
  if (top < el.scrollTop) el.scrollTop = top;
  else if (top + rowH.value + HEAD > el.scrollTop + el.clientHeight) el.scrollTop = top + rowH.value + HEAD - el.clientHeight;
}

function sortBy(key: string) {
  if (key === "traced") return;
  if (prefs.sort.key === key) prefs.sort.desc = !prefs.sort.desc;
  else prefs.sort = { key: key as SortKey, desc: false };
}

function formatDate(day: string | null): string {
  if (!day) return "";
  return new Date(`${day}T00:00:00`).toLocaleDateString(lang.value === "zh" ? "zh-CN" : "en-US", {
    year: "numeric",
    month: "short",
    day: "2-digit",
  });
}

/** Splits a name around the search hit so the hit can be emphasised. */
function nameParts(name: string): [string, string, string] {
  const q = filter.value.trim();
  const at = q ? name.toLowerCase().indexOf(q.toLowerCase()) : -1;
  return at < 0 ? [name, "", ""] : [name.slice(0, at), name.slice(at, at + q.length), name.slice(at + q.length)];
}

function select(id: string, e: { ctrlKey: boolean; shiftKey: boolean }) {
  const ids = rows.value.map((r) => r.app.id);
  if (e.shiftKey && anchor && ids.includes(anchor)) {
    const [from, to] = [ids.indexOf(anchor), ids.indexOf(id)].sort((a, b) => a - b);
    selected.clear();
    for (const i of ids.slice(from, to + 1)) selected.add(i);
    return;
  }
  if (e.ctrlKey) {
    if (selected.has(id)) selected.delete(id);
    else selected.add(id);
  } else {
    selected.clear();
    selected.add(id);
  }
  anchor = id;
}

function openContext(row: Row, e: MouseEvent) {
  if (!selected.has(row.app.id)) select(row.app.id, { ctrlKey: false, shiftKey: false });
  showMenu(e, actionItems.value);
}

function onListKey(e: KeyboardEvent) {
  const list = rows.value;
  if (!list.length) return;
  const lastId = selection.value.at(-1)?.id ?? anchor;
  const at = list.findIndex((r) => r.app.id === lastId);
  const row = list[at];
  let to = at;
  if (e.key === "ArrowDown") to = Math.min(at + 1, list.length - 1);
  else if (e.key === "ArrowUp") to = Math.max(at - 1, 0);
  else if (e.key === "Home") to = 0;
  else if (e.key === "End") to = list.length - 1;
  else if (e.key === "ArrowRight" && row) return setOpen(row, true);
  else if (e.key === "ArrowLeft" && row) return setOpen(row, false);
  else return;
  e.preventDefault();
  select(list[to].app.id, { ctrlKey: false, shiftKey: e.shiftKey });
  reveal(to);
}

const categoryName = computed(() => {
  const key = `cat.${category.value}` as MsgKey;
  return category.value === "recent" ? t(key, "").replace(/\s*[(（].*$/, "") : t(key);
});

const hasIcon = (a: App) => !brokenIcons.value.has(a.id);

/** Which installer built the selected program; read on demand, since it means opening its uninstaller. */
const installer = ref<InstallerInfo | null>(null);
watch(
  () => current.value?.id,
  (id) => {
    installer.value = null;
    if (!id) return;
    api
      .installerOf(id)
      .then((found) => {
        if (current.value?.id === id) installer.value = found;
      })
      .catch(() => {});
  },
  { immediate: true },
);
</script>

<template>
  <div class="body">
    <section ref="listEl" class="list" tabindex="0" @keydown="onListKey" @scroll.passive="scrollTop = ($event.target as HTMLElement).scrollTop">
      <table role="grid" aria-multiselectable="true" :aria-rowcount="rows.length" :style="{ '--row-h': `${rowH}px`, minWidth: `${cols.minWidth}px` }">
        <TableHead :cols="cols" :sort="prefs.sort" @sort="sortBy" />
        <tbody>
          <tr v-if="first" class="spacer" aria-hidden="true" :style="{ height: `${first * rowH}px` }"><td :colspan="span"></td></tr>
          <tr
            v-for="row in visible"
            :key="row.app.id"
            :aria-selected="selected.has(row.app.id)"
            :class="{ recent: prefs.highlightRecent && category !== 'recent' && isRecent(row.app) }"
            @click="select(row.app.id, $event)"
            @dblclick="start('uninstall')"
            @contextmenu.prevent.stop="openContext(row, $event)"
          >
            <td v-for="c in cols.columns" :key="c.key" :class="`col-${c.key}`">
              <div v-if="c.key === 'name'" class="name-cell" :style="{ paddingLeft: `${row.depth * 24}px` }">
                <button
                  v-if="row.kids && prefs.updates !== 'always'"
                  type="button"
                  class="twisty"
                  :class="{ open: row.open }"
                  :aria-expanded="row.open"
                  :aria-label="row.app.name"
                  @click.stop="setOpen(row, !row.open)"
                >
                  ›
                </button>
                <img v-if="hasIcon(row.app)" class="app-icon" :src="appIcon(row.app)" alt="" @error="brokenIcons.add(row.app.id)" />
                <span v-else class="app-icon placeholder" aria-hidden="true"></span>
                <span class="text" :title="row.app.name">
                  {{ nameParts(row.app.name)[0] }}<mark>{{ nameParts(row.app.name)[1] }}</mark
                  >{{ nameParts(row.app.name)[2] }}
                  <span v-if="prefs.mark32 && row.app.is_32bit" class="bits">{{ t("list.bit32") }}</span>
                  <span v-if="row.app.update" class="bits" :title="t('upd.availableOf', row.app.update)">↑</span>
                  <span v-if="row.app.broken" class="bits">{{ t("list.broken") }}</span>
                </span>
              </div>
              <template v-else-if="c.key === 'size'">{{ formatSize(sizeOf(row.app)) }}</template>
              <template v-else-if="c.key === 'date'">{{ formatDate(row.app.install_date) }}</template>
              <template v-else-if="c.key === 'publisher'">{{ row.app.publisher }}</template>
              <template v-else-if="c.key === 'version'">{{ row.app.version }}</template>
              <template v-else-if="c.key === 'update'">
                <span v-if="row.app.update" class="available">{{ row.app.update }}</span>
              </template>
              <span v-else-if="c.key === 'traced' && row.app.traced" class="traced-mark" role="img" :aria-label="t('col.traced')"></span>
            </td>
            <td v-if="cols.filler"></td>
          </tr>
          <tr v-if="last < rows.length" class="spacer" aria-hidden="true" :style="{ height: `${(rows.length - last) * rowH}px` }">
            <td :colspan="span"></td>
          </tr>
        </tbody>
      </table>
      <p v-if="loading && !rows.length" class="empty">{{ t("list.loading") }}</p>
      <p v-else-if="!rows.length && filter" class="empty">
        {{ t("list.noMatch", filter, categoryName) }}<br />
        <button type="button" class="btn ghost" @click="filter = ''">{{ t("list.reset") }}</button>
      </p>
      <p v-else-if="!rows.length" class="empty">{{ t("list.empty", categoryName) }}</p>
    </section>

    <aside class="panel" :class="{ closed: !prefs.details }" :inert="!prefs.details" aria-live="polite">
     <div class="panel-inner">
      <template v-if="current">
        <div class="panel-head">
          <img v-if="hasIcon(current)" class="app-icon" :src="appIcon(current)" alt="" />
          <span v-else class="app-icon placeholder" aria-hidden="true"></span>
          <h2>{{ current.name }}</h2>
        </div>
        <p v-if="current.broken" class="note">{{ t("panel.broken") }}</p>
        <p v-else-if="current.id.startsWith('TRACE:')" class="note">{{ t("panel.tracedOnly") }}</p>
        <p v-else-if="!canUninstall(current)" class="note">{{ t("panel.noUninstall") }}</p>
        <div class="panel-actions">
          <button v-if="current.broken" type="button" class="btn danger" @click="start('entry')">{{ t("act.removeEntry") }}</button>
          <button v-else-if="canUninstall(current)" type="button" class="btn danger" @click="start('uninstall')">{{ t("panel.uninstall") }}</button>
          <button v-if="current.modify_path" type="button" class="btn" @click="start('modify')">{{ t("panel.modify") }}</button>
          <button type="button" class="btn" @click="start('force')">{{ t("act.force") }}</button>
          <button type="button" class="btn" @click="askAbout(current)">{{ t("act.askAi") }}</button>
        </div>
        <dl class="props">
          <template v-if="SOURCES[current.source]">
            <dt>{{ t("col.source") }}</dt>
            <dd>{{ SOURCES[current.source] }}</dd>
          </template>
          <template v-if="current.version">
            <dt>{{ t("col.version") }}</dt>
            <dd>{{ current.version }}</dd>
          </template>
          <template v-if="current.publisher">
            <dt>{{ t("col.publisher") }}</dt>
            <dd>{{ current.publisher }}</dd>
          </template>
          <template v-if="current.install_date">
            <dt>{{ t("col.date") }}</dt>
            <dd>{{ formatDate(current.install_date) }}</dd>
          </template>
          <template v-if="sizeOf(current)">
            <dt>{{ t("col.size") }}</dt>
            <dd>{{ formatSize(sizeOf(current)) }}</dd>
          </template>
          <template v-if="installer && installer.kind !== 'unknown'">
            <dt>{{ t("panel.installer") }}</dt>
            <dd>{{ t(`inst.${installer.kind}` as MsgKey) }}</dd>
          </template>
        </dl>
        <button v-if="current.install_location" type="button" class="link" @click="jump.folder()">{{ current.install_location }}</button>
        <button v-if="current.url" type="button" class="link" @click="jump.website()">{{ current.url }}</button>
        <button v-if="current.reg_path" type="button" class="link" @click="jump.registry()">{{ t("panel.registry") }}</button>
      </template>
      <template v-else-if="selection.length > 1">
        <div class="panel-head">
          <h2>{{ t("panel.selected", selection.length) }}</h2>
        </div>
        <div class="panel-actions">
          <button v-if="selection.every(canUninstall)" type="button" class="btn danger" @click="start('uninstall')">{{ t("panel.uninstall") }}</button>
          <button type="button" class="btn" @click="start('force')">{{ t("act.forceBatch") }}</button>
          <button v-if="selection.every((a) => a.reg_path !== '')" type="button" class="btn" @click="start('entry')">
            {{ t("act.removeEntryBatch") }}
          </button>
        </div>
      </template>
      <p v-else class="empty">{{ t("panel.none") }}</p>
     </div>
    </aside>
  </div>
</template>
