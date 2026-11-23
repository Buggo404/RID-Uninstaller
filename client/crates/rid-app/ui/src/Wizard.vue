<script setup lang="ts">
import { save } from "@tauri-apps/plugin-dialog";
import { computed, onMounted, ref } from "vue";
import { report as showError } from "./actions";
import { type App, type Leftover, type Proc, type Report, api, formatSize, iconUrl, targetText } from "./api";
import { t } from "./i18n";
import { prefs } from "./prefs";
import type { JobMode } from "./store";

const props = defineProps<{ mode: JobMode; apps: App[]; ask?: boolean }>();
const emit = defineEmits<{ close: [changed: boolean] }>();

type Step = "confirm" | "running" | "ask" | "failed" | "scanning" | "review" | "removing" | "done";

const dialog = ref<HTMLDialogElement>();
const step = ref<Step>("confirm");
const understood = ref(false);
const dontAsk = ref(false);
const remember = ref(false);
const runningName = ref("");
const failed = ref<App[]>([]);
const scanIds = ref<string[]>([]);
const found = ref<(Leftover & { checked: boolean })[]>([]);
const procs = ref<Proc[]>([]);
const result = ref<Report | null>(null);
/// Per program: can it be uninstalled without its uninstaller showing anything?
const silent = ref<boolean[]>([]);
let changed = false;

const many = props.apps.length > 1;
const first = props.apps[0].name;
const label = many ? `${first} +${props.apps.length - 1}` : first;

const title = computed(() => {
  if (step.value !== "confirm" && props.mode !== "force") return label;
  if (props.mode === "force") return many ? t("wiz.forceMany", props.apps.length) : t("wiz.forceOne", first);
  if (props.mode === "entry") return many ? t("wiz.entryMany", props.apps.length) : t("wiz.entryOne", first);
  return many ? t("wiz.uninstallMany", props.apps.length) : t("wiz.uninstallOne", first);
});

const isFile = (l: Leftover) => l.target.kind === "dir" || l.target.kind === "file";
const groups = computed(() => [
  { title: t("wiz.files", found.value.filter(isFile).length), items: found.value.filter(isFile) },
  { title: t("wiz.registry", found.value.filter((l) => !isFile(l)).length), items: found.value.filter((l) => !isFile(l)) },
]);
const fileKb = computed(() => found.value.filter(isFile).reduce((sum, l) => sum + l.size_kb, 0));
const failures = computed(() => result.value?.log.filter((l) => !l.ok) ?? []);
const busy = computed(() => ["running", "removing", "scanning"].includes(step.value));

function close() {
  dialog.value?.close();
}

/// Any backend failure ends the wizard with the error shown; nothing continues on a guess.
async function guarded(work: () => Promise<void>) {
  try {
    await work();
  } catch (e) {
    showError(e);
    close();
  }
}

const runAll = (modify: boolean) =>
  guarded(async () => {
    step.value = "running";
    const done: string[] = [];
    // Silent ones first; those that need attention come last so the unattended part finishes on its own.
    const queue = props.apps.map((app, i) => ({ app, quiet: prefs.quietUninstall && silent.value[i] === true }));
    queue.sort((a, b) => Number(b.quiet) - Number(a.quiet));
    for (const { app, quiet } of queue) {
      runningName.value = app.name;
      const r = await api.uninstallRun(app.id, modify, quiet);
      changed = true;
      if (r.still_installed || r.cancelled) failed.value.push(app);
      else done.push(app.id);
    }
    if (modify) return close();
    scanIds.value = done;
    if (failed.value.length) step.value = "failed";
    else afterUninstall();
  });

function afterUninstall() {
  if (!scanIds.value.length || prefs.wizard === "off") return close();
  if (prefs.wizard === "auto") return scan();
  step.value = "ask";
}

function answer(yes: boolean) {
  if (remember.value) prefs.wizard = yes ? "auto" : "off";
  if (yes) scan();
  else close();
}

const scan = () =>
  guarded(async () => {
    step.value = "scanning";
    const ids = scanIds.value;
    found.value = (await api.scanLeftovers(ids)).map((l) => ({ ...l, checked: l.certain }));
    procs.value = await api.processes(ids);
    step.value = found.value.length ? "review" : "done";
  });

const kill = () =>
  guarded(async () => {
    const problems = await api.killProcesses(scanIds.value, procs.value.map((p) => p.pid));
    if (problems.length) showError(problems.join("\n"));
    procs.value = await api.processes(scanIds.value);
  });

const finish = () =>
  guarded(async () => {
    step.value = "removing";
    const targets = found.value.filter((l) => l.checked).map((l) => l.target);
    result.value = await api.removeLeftovers(label, scanIds.value, targets, prefs.permanentDelete);
    changed = true;
    step.value = "done";
  });

const removeEntries = () =>
  guarded(async () => {
    step.value = "removing";
    result.value = await api.removeEntries(label, props.apps.map((a) => a.id));
    changed = true;
    step.value = "done";
  });

function confirm() {
  if (dontAsk.value) prefs.confirmUninstall = false;
  if (props.mode === "uninstall") return runAll(false);
  if (props.mode === "entry") return removeEntries();
  scanIds.value = props.apps.map((a) => a.id);
  scan();
}

function forceFailed() {
  scanIds.value = [...scanIds.value, ...failed.value.map((a) => a.id)];
  scan();
}

function setGroup(items: { checked: boolean }[], checked: boolean) {
  for (const item of items) item.checked = checked;
}

async function saveLog() {
  const path = await save({ defaultPath: `${first}_UninstallLog.log`, filters: [{ name: t("wiz.logFilter"), extensions: ["log", "txt"] }] });
  if (!path || !result.value) return;
  const lines = result.value.log.map((l) => `${l.ok ? "OK    " : "FAILED"} ${l.target.kind.padEnd(9)} ${targetText(l.target)}  (${l.message})`);
  const pending = result.value.pending_reboot.map((p) => `REBOOT ${p}`);
  api.saveTextFile(path, [label, new Date().toISOString(), "", ...lines, ...pending, ""].join("\r\n")).catch(showError);
}

async function exportXml() {
  const path = await save({ defaultPath: `${first}_Traces.xml`, filters: [{ name: t("wiz.xmlFilter"), extensions: ["xml"] }] });
  if (!path) return;
  const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
  const items = found.value.map(
    (l) => `  <Item kind="${l.target.kind}" evidence="${l.evidence}" selected="${l.checked}" sizeKB="${l.size_kb}">${esc(targetText(l.target))}</Item>`,
  );
  const xml = `<?xml version="1.0" encoding="UTF-8"?>\n<Leftovers program="${esc(label)}">\n${items.join("\n")}\n</Leftovers>\n`;
  api.saveTextFile(path, xml).catch(showError);
}

onMounted(() =>
  guarded(async () => {
    dialog.value?.showModal();
    if (props.mode === "modify") return void runAll(true);
    if (props.mode !== "uninstall") return;
    silent.value = await api.quietSupport(props.apps.map((a) => a.id));
    // Only an ordinary uninstall can skip its question; force removal and entry removal always ask.
    if (!prefs.confirmUninstall && !props.ask && step.value === "confirm") confirm();
  }),
);
</script>

<template>
  <dialog ref="dialog" class="dialog wizard" aria-labelledby="wizard-title" @cancel="busy && $event.preventDefault()" @close="emit('close', changed)">
    <h2 id="wizard-title">{{ title }}</h2>

    <template v-if="step === 'confirm'">
      <ul class="app-cards">
        <li v-for="(a, i) in apps" :key="a.id">
          <img v-if="a.icon" class="app-icon" :src="iconUrl(a.icon)" alt="" />
          <span v-else class="app-icon placeholder" aria-hidden="true"></span>
          <span class="app-card-name">{{ a.name }}</span>
          <span v-if="mode === 'uninstall' && prefs.quietUninstall && silent[i] === false" class="chip">{{ t("wiz.needsYou") }}</span>
          <span class="app-card-meta">{{ [a.version, formatSize(a.size_kb)].filter(Boolean).join(" · ") }}</span>
        </li>
      </ul>
      <p v-if="mode === 'entry'" class="note">{{ t("wiz.entryInfo") }}</p>
      <template v-else-if="mode === 'force'">
        <p class="note">{{ t("wiz.forceInfo") }}</p>
        <label class="check"><input v-model="understood" type="checkbox" /> {{ t("wiz.forceCheck") }}</label>
      </template>
      <div class="dialog-buttons">
        <label v-if="mode === 'uninstall'" class="check"><input v-model="dontAsk" type="checkbox" /> {{ t("wiz.dontAsk") }}</label>
        <span class="spacer"></span>
        <button type="button" class="btn" @click="close">{{ t("btn.cancel") }}</button>
        <button type="button" class="btn danger" autofocus :disabled="mode === 'force' && !understood" @click="confirm">
          {{ mode === "uninstall" ? t("wiz.uninstall") : t("btn.yes") }}
        </button>
      </div>
    </template>

    <template v-else-if="step === 'running'">
      <p role="status">{{ t(mode === "modify" ? "wiz.modifying" : "wiz.running", runningName) }}</p>
      <div class="progress" role="progressbar"><i></i></div>
      <div class="dialog-buttons">
        <span class="note">{{ t("wiz.runningInfo") }}</span>
        <span class="spacer"></span>
        <button type="button" class="btn" :title="t('wiz.cancelInfo')" @click="api.uninstallCancel()">{{ t("wiz.cancelRun") }}</button>
      </div>
    </template>

    <template v-else-if="step === 'ask'">
      <p>{{ t("wiz.ask") }}</p>
      <div class="dialog-buttons">
        <label class="check"><input v-model="remember" type="checkbox" /> {{ t("wiz.remember") }}</label>
        <span class="spacer"></span>
        <button type="button" class="btn" @click="answer(false)">{{ t("btn.no") }}</button>
        <button type="button" class="btn primary" autofocus @click="answer(true)">{{ t("btn.yes") }}</button>
      </div>
    </template>

    <template v-else-if="step === 'failed'">
      <p>{{ t("wiz.failed", failed.map((a) => a.name).join(", ")) }}</p>
      <p class="note">{{ t("wiz.failedAsk") }}</p>
      <div class="dialog-buttons">
        <button type="button" class="btn" @click="forceFailed">{{ t("btn.yes") }}</button>
        <button type="button" class="btn primary" autofocus @click="afterUninstall">{{ t("btn.no") }}</button>
      </div>
    </template>

    <template v-else-if="step === 'scanning' || step === 'removing'">
      <p role="status">{{ t(step === "scanning" ? "wiz.scanning" : "wiz.removing") }}</p>
      <div class="progress" role="progressbar"><i></i></div>
    </template>

    <template v-else-if="step === 'review'">
      <p class="note">{{ t("wiz.review") }}</p>
      <div v-if="procs.length" class="notice" role="alert">
        <p>{{ t("wiz.procs") }}</p>
        <ul>
          <li v-for="p in procs" :key="p.pid">{{ p.path }} ({{ p.pid }})</li>
        </ul>
        <button type="button" class="btn" @click="kill">{{ t("wiz.kill") }}</button>
      </div>
      <div class="traces">
        <section v-for="g in groups" v-show="g.items.length" :key="g.title">
          <label class="check trace-group">
            <input
              type="checkbox"
              :checked="g.items.every((i) => i.checked)"
              :indeterminate="g.items.some((i) => i.checked) && !g.items.every((i) => i.checked)"
              @change="setGroup(g.items, ($event.target as HTMLInputElement).checked)"
            />
            {{ g.title }}
          </label>
          <label v-for="(l, i) in g.items" :key="i" class="trace">
            <input v-model="l.checked" type="checkbox" />
            <span class="trace-path">
              <template v-if="l.target.kind === 'service'">{{ t("kind.service") }}: </template>{{ targetText(l.target) }}
            </span>
            <span v-if="l.documents" class="chip warn">{{ t("wiz.documents", l.documents) }}</span>
            <span v-else class="chip" :class="{ warn: !l.certain }" :title="l.certain ? '' : t('wiz.uncertain')">{{ t(`ev.${l.evidence}`) }}</span>
            <span class="trace-size">{{ formatSize(l.size_kb) }}</span>
          </label>
        </section>
      </div>
      <div class="dialog-buttons">
        <span class="note">{{ t("wiz.summary", groups[0].items.length, formatSize(fileKb) || "0 KB", groups[1].items.length) }}</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="exportXml">{{ t("wiz.exportXml") }}</button>
        <button type="button" class="btn" @click="close">{{ t("btn.cancel") }}</button>
        <button type="button" class="btn danger" :disabled="!found.some((l) => l.checked)" @click="finish">{{ t("wiz.finish") }}</button>
      </div>
    </template>

    <template v-else>
      <p v-if="!result" role="status">{{ t("wiz.nothing") }}</p>
      <template v-else>
        <p role="status">{{ t("wiz.done", result.log.length - failures.length) }}</p>
        <div v-if="failures.length" class="notice" role="alert">
          <p>{{ t("wiz.doneFailed", failures.length) }}</p>
          <ul>
            <li v-for="(f, i) in failures" :key="i">{{ targetText(f.target) }}: {{ f.message }}</li>
          </ul>
        </div>
        <div v-if="result.pending_reboot.length" class="notice">
          <p>{{ t("wiz.pending") }}</p>
          <ul>
            <li v-for="p in result.pending_reboot" :key="p">{{ p }}</li>
          </ul>
        </div>
        <p v-if="result.batch" class="note">{{ t("wiz.undo") }}</p>
      </template>
      <div class="dialog-buttons">
        <button v-if="result" type="button" class="btn ghost" @click="saveLog">{{ t("wiz.saveLog") }}</button>
        <span class="spacer"></span>
        <button type="button" class="btn primary" autofocus @click="close">{{ t("btn.close") }}</button>
      </div>
    </template>
  </dialog>
</template>
