<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { report } from "./actions";
import { SOURCES, type UpdateItem, api } from "./api";
import { useColumns } from "./columns";
import { t } from "./i18n";
import { prefs } from "./prefs";
import { filter, refreshUpdates, upgradeJob, upgradePackages, upgrades } from "./store";
import TableHead from "./TableHead.vue";
import { showMenu } from "./ui";

const cols = useColumns("updates", () => [
  { key: "name", label: t("col.name"), width: 0 },
  { key: "source", label: t("col.source"), width: 120 },
  { key: "installed", label: t("upd.installed"), width: 110 },
  { key: "available", label: t("upd.available"), width: 110 },
]);

const chosen = ref(new Set<string>());

const rows = computed(() => {
  const q = filter.value.trim().toLowerCase();
  return upgrades.items.filter((u) => !q || u.name.toLowerCase().includes(q) || u.id.toLowerCase().includes(q));
});
const selected = computed(() => rows.value.filter((u) => chosen.value.has(u.id)));

function toggle(u: UpdateItem) {
  if (chosen.value.has(u.id)) chosen.value.delete(u.id);
  else chosen.value.add(u.id);
}

async function run(items: UpdateItem[]) {
  await upgradePackages(items).catch(report);
  const failed = upgradeJob.failures;
  if (failed.length) report(failed.map((f) => `${f.name}: ${f.message}`).join("\n"));
}

function openContext(u: UpdateItem, e: MouseEvent) {
  showMenu(e, [
    { label: t("upd.one"), disabled: upgradeJob.running, run: () => run([u]) },
    { label: t("upd.selected", selected.value.length), disabled: !selected.value.length || upgradeJob.running, run: () => run(selected.value) },
    { sep: true },
    { label: t("upd.all", upgrades.items.length), disabled: !upgrades.items.length || upgradeJob.running, run: () => run(upgrades.items) },
  ]);
}

onMounted(() => {
  if (!upgrades.items.length) refreshUpdates().catch(report);
});
</script>

<template>
  <div class="body">
    <section class="list" tabindex="0">
      <table role="grid" :aria-rowcount="rows.length" :style="{ '--row-h': `${prefs.compact ? 30 : 40}px`, minWidth: `${cols.minWidth}px` }">
        <TableHead :cols="cols" />
        <tbody>
          <tr
            v-for="u in rows"
            :key="u.id"
            :aria-selected="chosen.has(u.id)"
            @click="toggle(u)"
            @dblclick="run([u])"
            @contextmenu.prevent.stop="openContext(u, $event)"
          >
            <td v-for="c in cols.columns" :key="c.key" :class="`col-${c.key}`">
              <div v-if="c.key === 'name'" class="name-cell">
                <input type="checkbox" :checked="chosen.has(u.id)" :aria-label="u.name" @click.stop @change="toggle(u)" />
                <span class="text" :title="u.name">{{ u.name }}</span>
              </div>
              <template v-else-if="c.key === 'source'">{{ SOURCES[u.source] ?? u.source }}</template>
              <template v-else-if="c.key === 'installed'">{{ u.installed }}</template>
              <template v-else>{{ u.available }}</template>
            </td>
            <td v-if="cols.filler"></td>
          </tr>
        </tbody>
      </table>
      <p v-if="upgrades.loading && !rows.length" class="empty">{{ t("upd.checking") }}</p>
      <p v-else-if="!rows.length && filter" class="empty">{{ t("list.noMatch", filter, t("nav.updates")) }}</p>
      <p v-else-if="!rows.length" class="empty">{{ t("upd.none") }}</p>
    </section>

    <aside class="panel" :class="{ closed: !prefs.details }" :inert="!prefs.details" aria-live="polite">
      <div class="panel-inner">
        <div class="panel-head">
          <h2>{{ t("nav.updates") }}</h2>
        </div>
        <div class="panel-actions">
          <button type="button" class="btn primary" :disabled="upgradeJob.running || !selected.length" @click="run(selected)">
            {{ t("upd.selected", selected.length) }}
          </button>
          <button type="button" class="btn" :disabled="upgradeJob.running || !upgrades.items.length" @click="run(upgrades.items)">
            {{ t("upd.all", upgrades.items.length) }}
          </button>
          <button v-if="upgradeJob.running" type="button" class="btn" @click="api.uninstallCancel()">{{ t("wiz.cancelRun") }}</button>
        </div>
        <p v-if="upgradeJob.running" class="note">{{ t("upd.running", upgradeJob.done, upgradeJob.total) }} {{ upgradeJob.current }}</p>
        <p v-if="upgrades.warnings.length" class="note" :title="upgrades.warnings.join('\n')">
          {{ t("status.warnings", upgrades.warnings.length) }}
        </p>
      </div>
    </aside>
  </div>
</template>
