<script setup lang="ts">
import { open } from "@tauri-apps/plugin-dialog";
import { computed, onMounted, reactive, ref, watch } from "vue";
import { report } from "./actions";
import { type StartupItem, api, iconUrl } from "./api";
import { t } from "./i18n";
import { prefs } from "./prefs";
import RSelect from "./RSelect.vue";
import { filter, refreshStartup, startup, startupGroupOf } from "./store";
import { confirmBox } from "./ui";

const selectedKey = ref<string | null>(null);
const brokenIcons = ref(new Set<string>());
const addDialog = ref<HTMLDialogElement>();
const form = reactive({ to: "hkcu_run", name: "", exe: "", args: "", highest: false });

const keyOf = (i: StartupItem) => `${i.source}|${i.name}`;

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
      <table role="grid" style="--row-h: 40px">
        <colgroup>
          <col />
          <col style="width: 110px" />
          <col style="width: 190px" />
          <col style="width: 200px" />
        </colgroup>
        <thead>
          <tr>
            <th scope="col"><button type="button">{{ t("col.name") }}</button></th>
            <th scope="col"><button type="button">{{ t("st.colStatus") }}</button></th>
            <th scope="col"><button type="button">{{ t("st.colSource") }}</button></th>
            <th scope="col"><button type="button">{{ t("st.colCompany") }}</button></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="i in rows" :key="keyOf(i)" :aria-selected="keyOf(i) === selectedKey" @click="selectedKey = keyOf(i)">
            <td>
              <div class="name-cell">
                <input
                  type="checkbox"
                  :checked="i.status !== 'disabled'"
                  :disabled="!i.can_disable"
                  :aria-label="t(i.status === 'disabled' ? 'st.enable' : 'st.disable')"
                  @click.stop
                  @change="toggle(i)"
                />
                <img v-if="i.exe && !brokenIcons.has(keyOf(i))" class="app-icon" :src="iconUrl(i.exe)" alt="" @error="brokenIcons.add(keyOf(i))" />
                <span v-else class="app-icon placeholder" aria-hidden="true"></span>
                <span class="text" :title="i.title">
                  {{ i.title }} <span v-if="prefs.mark32 && i.is_32bit" class="bits">{{ t("list.bit32") }}</span>
                </span>
              </div>
            </td>
            <td><span class="status-dot" :class="i.status"></span>{{ t(`st.${i.status}`) }}</td>
            <td class="col-size">{{ t(`st.${i.source}`) }}</td>
            <td class="col-size">{{ i.company }}</td>
          </tr>
        </tbody>
      </table>
      <p v-if="startup.loading && !rows.length" class="empty">{{ t("list.loading") }}</p>
      <p v-else-if="!rows.length" class="empty">{{ t("st.empty") }}</p>
    </section>

    <aside class="panel" aria-live="polite">
      <template v-if="current">
        <div class="panel-head">
          <h2>{{ current.title }}</h2>
        </div>
        <div class="panel-actions">
          <button v-if="current.can_disable" type="button" class="btn" @click="toggle(current)">
            {{ t(current.status === "disabled" ? "st.enable" : "st.disable") }}
          </button>
          <button v-if="current.source !== 'store_app'" type="button" class="btn danger" @click="remove(current)">{{ t("st.delete") }}</button>
        </div>
        <dl class="props">
          <dt>{{ t("st.colStatus") }}</dt>
          <dd>{{ t(`st.${current.status}`) }}</dd>
          <dt>{{ t("st.colSource") }}</dt>
          <dd>{{ t(`st.${current.source}`) }}</dd>
        </dl>
        <p v-if="current.command" class="command">{{ current.command }}</p>
      </template>
      <p v-else class="empty">{{ t("panel.none") }}</p>
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
