<script setup lang="ts">
import { open } from "@tauri-apps/plugin-dialog";
import { computed, onMounted, reactive, ref, watch } from "vue";
import { report } from "./actions";
import { type StartupItem, api, iconUrl } from "./api";
import { useColumns } from "./columns";
import { t } from "./i18n";
import { prefs } from "./prefs";
import RSelect from "./RSelect.vue";
import { filter, refreshStartup, startup, startupGroupOf } from "./store";
import TableHead from "./TableHead.vue";
import { asking, confirmBox, showMenu } from "./ui";

const selectedKey = ref<string | null>(null);
const brokenIcons = ref(new Set<string>());
const addDialog = ref<HTMLDialogElement>();
const form = reactive({ to: "hkcu_run", name: "", exe: "", args: "", highest: false });

const cols = useColumns("startup", () => [
  { key: "name", label: t("col.name"), width: 0 },
  { key: "status", label: t("st.colStatus"), width: 110 },
  { key: "source", label: t("st.colSource"), width: 190 },
  { key: "company", label: t("st.colCompany"), width: 200 },
]);

const keyOf = (i: StartupItem) => `${i.source}|${i.name}`;
const hasIcon = (i: StartupItem) => i.exe !== "" && !brokenIcons.value.has(keyOf(i));
/** Store tasks carry their app's logo in `exe`; only a real program has a folder to open. */
const folderOf = (i: StartupItem) => (i.source === "store_app" ? "" : i.exe.replace(/\\[^\\]*$/, ""));

const rows = computed(() => {
  const q = filter.value.trim().toLowerCase();
  return startup.items.filter(
    (i) =>
      (startup.group === "all" || startupGroupOf(i) === startup.group) &&
      (!q || i.title.toLowerCase().includes(q) || i.company.toLowerCase().includes(q)),
  );
});
const current = computed(() => rows.value.find((i) => keyOf(i) === selectedKey.value) ?? null);

const targets = computed(() => [
  { value: "hkcu_run", label: t("st.toHkcu") },
  { value: "hklm_run", label: t("st.toHklm") },
  { value: "folder_user", label: t("st.toFolderUser") },
  { value: "folder_common", label: t("st.toFolderCommon") },
  { value: "task", label: t("st.toTask") },
]);

const reload = () => refreshStartup().catch(report);

async function act(work: () => Promise<unknown>) {
  try {
    await work();
  } catch (e) {
    report(e);
  }
  reload();
}

const toggle = (i: StartupItem) => act(() => api.startupSetEnabled(i.source, i.name, i.status === "disabled"));

const remove = (i: StartupItem) =>
  act(async () => {
    const question = t(i.source === "task" ? "st.confirmDeleteTask" : "st.confirmDelete", i.title);
    if (await confirmBox(question)) await api.startupDelete(i.source, i.name);
  });

const ask = (i: StartupItem) =>
  (asking.value = {
    title: i.title,
    icon: hasIcon(i) ? iconUrl(i.exe) : undefined,
    question: { feature: "startup", title: i.title, command: i.command, exe: i.exe, company: i.company, source: t(`st.${i.source}`), status: i.status },
  });

function openContext(i: StartupItem, e: MouseEvent) {
  selectedKey.value = keyOf(i);
  showMenu(e, [
    { label: t(i.status === "disabled" ? "st.enable" : "st.disable"), key: "Space", disabled: !i.can_disable, run: () => toggle(i) },
    { label: t("st.delete"), key: "Shift+Del", disabled: i.source === "store_app", run: () => remove(i) },
    { sep: true },
    { label: t("act.askAi"), run: () => ask(i) },
    { label: t("st.openLocation"), disabled: !folderOf(i), run: () => api.openFolder(folderOf(i)).catch(report) },
    { label: t("st.copyCommand"), disabled: !i.command, run: () => navigator.clipboard.writeText(i.command).catch(report) },
  ]);
}

async function browse() {
  const picked = await open({ multiple: false, filters: [{ name: t("st.exeFilter"), extensions: ["exe", "bat", "cmd"] }] });
  if (typeof picked !== "string") return;
  form.exe = picked;
  // The file name without extension is the obvious default for the entry's name.
  if (!form.name) form.name = picked.replace(/^.*[\\/]/, "").replace(/\.[^.]+$/, "");
}

function submit() {
  const to = form.to === "task" && form.highest ? "task_elevated" : form.to;
  addDialog.value?.close();
  act(() => api.startupAdd(to, form.name, form.exe, form.args));
}

function onKey(e: KeyboardEvent) {
  const i = current.value;
  const at = rows.value.findIndex((r) => keyOf(r) === selectedKey.value);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const to = Math.max(0, Math.min(rows.value.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
    if (rows.value[to]) selectedKey.value = keyOf(rows.value[to]);
  } else if (i && e.key === "Delete" && e.shiftKey && i.source !== "store_app") {
    e.preventDefault();
    remove(i);
  } else if (i && e.key === " " && i.can_disable) {
    e.preventDefault();
    toggle(i);
  }
}

watch(
  () => startup.adding,
  (adding) => {
    if (!adding) return;
    Object.assign(form, { to: "hkcu_run", name: "", exe: "", args: "", highest: false });
    addDialog.value?.showModal();
  },
);

onMounted(reload);
</script>

<template>
  <div class="body">
    <section class="list" tabindex="0" @keydown="onKey">
      <table role="grid" :style="{ '--row-h': `${prefs.compact ? 30 : 40}px`, minWidth: `${cols.minWidth}px` }">
        <TableHead :cols="cols" />
        <tbody>
          <tr
            v-for="i in rows"
            :key="keyOf(i)"
            :aria-selected="keyOf(i) === selectedKey"
            @click="selectedKey = keyOf(i)"
            @contextmenu.prevent.stop="openContext(i, $event)"
          >
            <td v-for="c in cols.columns" :key="c.key" :class="`col-${c.key}`">
              <div v-if="c.key === 'name'" class="name-cell">
                <input
                  type="checkbox"
                  :checked="i.status !== 'disabled'"
                  :disabled="!i.can_disable"
                  :aria-label="t(i.status === 'disabled' ? 'st.enable' : 'st.disable')"
                  @click.stop
                  @change="toggle(i)"
                />
                <img v-if="hasIcon(i)" class="app-icon" :src="iconUrl(i.exe)" alt="" @error="brokenIcons.add(keyOf(i))" />
                <span v-else class="app-icon placeholder" aria-hidden="true"></span>
                <span class="text" :title="i.title">
                  {{ i.title }} <span v-if="prefs.mark32 && i.is_32bit" class="bits">{{ t("list.bit32") }}</span>
                </span>
              </div>
              <template v-else-if="c.key === 'status'"><span class="status-dot" :class="i.status"></span>{{ t(`st.${i.status}`) }}</template>
              <template v-else-if="c.key === 'source'">{{ t(`st.${i.source}`) }}</template>
              <template v-else>{{ i.company }}</template>
            </td>
            <td v-if="cols.filler"></td>
          </tr>
        </tbody>
      </table>
      <p v-if="startup.loading && !rows.length" class="empty">{{ t("list.loading") }}</p>
      <p v-else-if="!rows.length" class="empty">{{ t("st.empty") }}</p>
    </section>

    <aside class="panel" :class="{ closed: !prefs.details }" :inert="!prefs.details" aria-live="polite">
      <div class="panel-inner">
        <template v-if="current">
          <div class="panel-head">
            <img v-if="hasIcon(current)" class="app-icon" :src="iconUrl(current.exe)" alt="" />
            <span v-else class="app-icon placeholder" aria-hidden="true"></span>
            <h2>{{ current.title }}</h2>
          </div>
          <div class="panel-actions">
            <button v-if="current.can_disable" type="button" class="btn" @click="toggle(current)">
              {{ t(current.status === "disabled" ? "st.enable" : "st.disable") }}
            </button>
            <button v-if="current.source !== 'store_app'" type="button" class="btn danger" @click="remove(current)">{{ t("st.delete") }}</button>
            <button type="button" class="btn" @click="ask(current)">{{ t("act.askAi") }}</button>
          </div>
          <dl class="props">
            <dt>{{ t("st.colStatus") }}</dt>
            <dd>{{ t(`st.${current.status}`) }}</dd>
            <dt>{{ t("st.colSource") }}</dt>
            <dd>{{ t(`st.${current.source}`) }}</dd>
            <template v-if="current.company">
              <dt>{{ t("st.colCompany") }}</dt>
              <dd>{{ current.company }}</dd>
            </template>
          </dl>
          <button v-if="folderOf(current)" type="button" class="link" @click="api.openFolder(folderOf(current)).catch(report)">{{ folderOf(current) }}</button>
          <p v-if="current.command" class="command">{{ current.command }}</p>
        </template>
        <p v-else class="empty">{{ t("panel.none") }}</p>
      </div>
    </aside>

    <dialog ref="addDialog" class="dialog" aria-labelledby="add-title" @close="startup.adding = false">
      <h2 id="add-title">{{ t("st.addTitle") }}</h2>
      <form @submit.prevent="submit">
        <div class="form">
          <span>{{ t("st.type") }}</span>
          <RSelect v-model="form.to" :options="targets" />
          <label for="st-exe">{{ t("st.path") }}</label>
          <div class="with-button">
            <input id="st-exe" v-model="form.exe" class="input" type="text" required spellcheck="false" />
            <button type="button" class="btn" @click="browse">{{ t("st.browse") }}</button>
          </div>
          <label for="st-name">{{ t("st.name") }}</label>
          <input id="st-name" v-model="form.name" class="input" type="text" required pattern='[^\\/:*?"<>|]+' />
          <label for="st-args">{{ t("st.args") }}</label>
          <input id="st-args" v-model="form.args" class="input" type="text" spellcheck="false" />
        </div>
        <label v-if="form.to === 'task'" class="check"><input v-model="form.highest" type="checkbox" /> {{ t("st.highest") }}</label>
        <div class="dialog-buttons">
          <span class="spacer"></span>
          <button type="button" class="btn" @click="addDialog?.close()">{{ t("btn.cancel") }}</button>
          <button type="submit" class="btn primary">{{ t("btn.ok") }}</button>
        </div>
      </form>
    </dialog>
  </div>
</template>
