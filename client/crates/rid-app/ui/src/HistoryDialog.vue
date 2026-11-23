<script setup lang="ts">
import { onMounted, ref } from "vue";
import { report } from "./actions";
import { type BatchInfo, api, formatSize } from "./api";
import { lang, t } from "./i18n";
import { confirmBox, notify } from "./ui";

const emit = defineEmits<{ close: [changed: boolean] }>();
const dialog = ref<HTMLDialogElement>();
const batches = ref<BatchInfo[]>([]);
const busy = ref(false);
let changed = false;

const load = () => api.quarantineList().then((b) => (batches.value = b), report);

const when = (b: BatchInfo) => new Date(b.created * 1000).toLocaleString(lang.value === "zh" ? "zh-CN" : "en-US");

async function act(work: () => Promise<void>) {
  busy.value = true;
  try {
    await work();
    changed = true;
  } catch (e) {
    report(e);
  }
  busy.value = false;
  load();
}

const restore = (b: BatchInfo) =>
  act(async () => {
    const problems = await api.quarantineRestore(b.id);
    const text = problems.length ? `${t("hist.partly")}\n\n${problems.join("\n")}` : t("hist.restored");
    await notify(text);
  });

/** Files a running program has loaded are deleted by Windows at the next restart; the user is told which. */
const inUse = (files: string[]) => files.length && notify(`\n\n`);

const purge = (b: BatchInfo) =>
  act(async () => {
    if (await confirmBox(t("hist.confirmDelete", b.label))) inUse(await api.quarantinePurge(b.id));
  });

const clear = () =>
  act(async () => {
    if (!(await confirmBox(t("hist.confirmClear", batches.value.length)))) return;
    const waiting: string[] = [];
    for (const b of batches.value) waiting.push(...(await api.quarantinePurge(b.id)));
    inUse(waiting);
  });

onMounted(() => {
  dialog.value?.showModal();
  load();
});
</script>

<template>
  <dialog ref="dialog" class="dialog wizard" aria-labelledby="history-title" @close="emit('close', changed)">
    <h2 id="history-title">{{ t("hist.title") }}</h2>
    <p v-if="!batches.length" class="note">{{ t("hist.empty") }}</p>
    <ul v-else class="batches">
      <li v-for="b in batches" :key="b.id">
        <div>
          <strong>{{ b.label }}</strong>
          <div class="note">{{ when(b) }} · {{ t("hist.info", b.items, formatSize(b.size_kb) || "0 KB") }}</div>
        </div>
        <button type="button" class="btn" :disabled="busy" @click="restore(b)">{{ t("hist.restore") }}</button>
        <button type="button" class="btn ghost" :disabled="busy" @click="purge(b)">{{ t("hist.delete") }}</button>
      </li>
    </ul>
    <form method="dialog" class="dialog-buttons">
      <button v-if="batches.length" type="button" class="btn" :disabled="busy" @click="clear">{{ t("hist.clear") }}</button>
      <span class="spacer"></span>
      <button class="btn primary" autofocus>{{ t("btn.close") }}</button>
    </form>
  </dialog>
</template>
