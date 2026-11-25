<script setup lang="ts">
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { nextTick, onMounted, ref } from "vue";
import { report } from "./actions";
import { type App, type InstallerInfo, type TraceEvent, type TraceResult, api, targetText } from "./api";
import { type MsgKey, t } from "./i18n";

const props = defineProps<{ appendTo?: App; initialPath?: string }>();
const emit = defineEmits<{ close: [changed: boolean] }>();

const dialog = ref<HTMLDialogElement>();
const logEl = ref<HTMLElement>();
const step = ref<"setup" | "running" | "result">("setup");
const path = ref(props.initialPath ?? "");
const args = ref("");
const log = ref<string[]>([]);
const result = ref<TraceResult | null>(null);
const mainId = ref("");
const customName = ref("");
const busy = ref(false);
const hint = ref<InstallerInfo | null>(null);
let changed = false;

const close = () => dialog.value?.close();

async function guarded(work: () => Promise<void>) {
  busy.value = true;
  try {
    await work();
  } catch (e) {
    report(e);
  }
  busy.value = false;
}

async function browse() {
  const picked = await open({ multiple: false, filters: [{ name: t("tr.setupFilter"), extensions: ["exe", "msi", "msp"] }] });
  if (typeof picked === "string") await use(picked);
}

/** Fills in the silent arguments of a recognised setup file; they stay editable. */
async function use(file: string) {
  path.value = file;
  hint.value = await api.installerHint(file).catch(() => null);
  if (!args.value.trim() && hint.value?.silent_install.length) args.value = hint.value.silent_install.join(" ");
}

function line(e: TraceEvent): string {
  const label = t((e.kind === "created" && e.dir ? "tr.createdDir" : `tr.${e.kind}`) as MsgKey);
  if (e.kind === "created" || e.kind === "deleted") return `${label}  ${e.path}`;
  if (e.kind === "renamed") return `${label}  ${e.from} → ${e.to}`;
  if (e.kind === "key_created" || e.kind === "key_deleted") return `${label}  ${e.key}`;
  if (e.kind === "value_set" || e.kind === "value_deleted") return `${label}  ${e.key} → ${e.name}`;
  return e.kind === "process_started" ? `${label}  ${e.image} (${e.pid})` : `${label}  (${e.pid})`;
}

const run = () =>
  guarded(async () => {
    step.value = "running";
    log.value = [];
    const unlisten = await listen<TraceEvent>("trace-event", ({ payload }) => {
      // shortcut: the on-screen log keeps the last 2000 lines; the recording itself is complete.
      if (log.value.push(line(payload)) > 2000) log.value.shift();
      nextTick(() => logEl.value?.scrollTo(0, logEl.value.scrollHeight));
    });
    try {
      result.value = await api.traceRun(path.value, args.value);
      mainId.value = result.value.new_apps[0]?.id ?? "";
      changed = true;
      step.value = "result";
    } catch (e) {
      step.value = "setup";
      throw e;
    } finally {
      unlisten();
    }
  });

const save = () =>
  guarded(async () => {
    const r = result.value!;
    if (props.appendTo) await api.traceSave(props.appendTo.id, null, [], true);
    else if (r.new_apps.length) {
      const dependents = r.new_apps.map((a) => a.id).filter((id) => id !== mainId.value);
      await api.traceSave(mainId.value, null, dependents, false);
    } else await api.traceSave(null, customName.value, [], false);
    close();
  });

const revert = () =>
  guarded(async () => {
    const r = await api.traceRevert();
    const failed = r.log.filter((l) => !l.ok);
    if (failed.length) report(failed.map((f) => `${targetText(f.target)}: ${f.message}`).join("\n"));
    close();
  });

onMounted(() => {
  dialog.value?.showModal();
  if (path.value) void use(path.value);
});
</script>

<template>
  <dialog
    ref="dialog"
    class="dialog wizard"
    aria-labelledby="trace-title"
    @cancel="step === 'running' && $event.preventDefault()"
    @close="emit('close', changed)"
  >
    <h2 id="trace-title">{{ appendTo ? t("tr.titleAppend", appendTo.name) : t("tr.title") }}</h2>

    <template v-if="step === 'setup'">
      <form @submit.prevent="run">
        <div class="form">
          <label for="tr-path">{{ t("tr.path") }}</label>
          <div class="with-button">
            <input id="tr-path" v-model="path" class="input" type="text" required spellcheck="false" />
            <button type="button" class="btn" @click="browse">{{ t("st.browse") }}</button>
          </div>
          <label for="tr-args">{{ t("tr.args") }}</label>
          <input id="tr-args" v-model="args" class="input" type="text" spellcheck="false" />
          <p v-if="hint && hint.kind !== 'unknown'" class="note">{{ t("tr.detected", t(`inst.${hint.kind}` as MsgKey)) }}</p>
        </div>
        <div class="dialog-buttons">
          <span class="note">{{ t("tr.hint") }}</span>
          <span class="spacer"></span>
          <button type="button" class="btn" @click="close">{{ t("btn.cancel") }}</button>
          <button type="submit" class="btn primary" :disabled="busy">
            {{ t("tr.install") }}
          </button>
        </div>
      </form>
    </template>

    <template v-else-if="step === 'running'">
      <p>{{ t("tr.progress") }}</p>
      <div ref="logEl" class="trace-log" role="log" aria-live="off" tabindex="0">
        <div v-for="(entry, i) in log" :key="i">{{ entry }}</div>
      </div>
      <div class="dialog-buttons">
        <button type="button" class="btn" @click="api.uninstallCancel()">{{ t("tr.stop") }}</button>
      </div>
    </template>

    <template v-else-if="result">
      <p role="status">{{ t("tr.finished", result.items.length) }}</p>
      <div v-if="result.dropped" class="notice" role="alert">{{ t("tr.dropped", result.dropped) }}</div>
      <p v-if="!result.items.length">{{ t("tr.nothing") }}</p>
      <template v-else-if="!appendTo">
        <p v-if="result.new_apps.length === 1">{{ t("tr.oneEntry", result.new_apps[0].name) }}</p>
        <fieldset v-else-if="result.new_apps.length > 1">
          <legend>{{ t("tr.manyEntries") }}</legend>
          <label v-for="a in result.new_apps" :key="a.id" class="check"><input v-model="mainId" type="radio" :value="a.id" /> {{ a.name }}</label>
        </fieldset>
        <label v-else class="field">
          <span>{{ t("tr.noEntry") }}</span>
          <input v-model="customName" class="input" type="text" pattern='[^\\/:*?"<>|]+' />
        </label>
      </template>
      <div class="traces">
        <div v-for="(item, i) in result.items" :key="i" class="trace-path">{{ targetText(item) }}</div>
      </div>
      <p v-if="result.items.length && !result.new_apps.length && !appendTo" class="note">{{ t("tr.revertInfo") }}</p>
      <div class="dialog-buttons">
        <button
          v-if="result.items.length"
          type="button"
          class="btn primary"
          :disabled="busy || (!appendTo && !result.new_apps.length && !customName.trim())"
          @click="save"
        >
          {{ t("tr.save") }}
        </button>
        <button type="button" class="btn" :disabled="busy" @click="close">{{ result.items.length ? t("tr.discard") : t("btn.close") }}</button>
        <button v-if="result.items.length && !result.new_apps.length && !appendTo" type="button" class="btn" :disabled="busy" @click="revert">
          {{ t("tr.revert") }}
        </button>
      </div>
    </template>
  </dialog>
</template>
