<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { report } from "./actions";
import { api } from "./api";
import { type MsgKey, t } from "./i18n";
import { prefs } from "./prefs";
import RSelect from "./RSelect.vue";

const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();
const contextMenu = ref(false);

// A new group of settings is one more entry here and one more block in the template.
const categories = ["interface", "list", "uninstaller", "update"] as const;
const category = ref<(typeof categories)[number]>("interface");

function toggleContextMenu() {
  const want = !contextMenu.value;
  api.setContextMenu(want, t("shell.trace"), t("shell.uninstall")).then(() => (contextMenu.value = want), report);
}

const languages = computed(() => [
  { value: "auto" as const, label: t("view.langAuto") },
  { value: "zh" as const, label: "简体中文" },
  { value: "en" as const, label: "English" },
  { value: "pt" as const, label: "Português (Brasil)" },
]);
const themes = computed(() => [
  { value: "auto" as const, label: t("prefs.schemeAuto") },
  { value: "light" as const, label: t("prefs.schemeLight") },
  { value: "dark" as const, label: t("prefs.schemeDark") },
]);
const updates = computed(() => [
  { value: "collapsed" as const, label: t("prefs.updatesCollapsed") },
  { value: "expanded" as const, label: t("prefs.updatesExpanded") },
  { value: "always" as const, label: t("prefs.updatesAlways") },
]);
const wizard = computed(() => [
  { value: "ask" as const, label: t("prefs.wizardAsk") },
  { value: "auto" as const, label: t("prefs.wizardAuto") },
  { value: "off" as const, label: t("prefs.wizardOff") },
]);

const retention = computed(() => (["7", "30", "90", "never"] as const).map((value) => ({ value, label: t(`prefs.q${value}`) })));

onMounted(() => {
  dialog.value?.showModal();
  api.contextMenuEnabled().then((on) => (contextMenu.value = on), report);
});
</script>

<template>
  <dialog ref="dialog" class="dialog prefs" aria-labelledby="prefs-title" @close="emit('close')">
    <div class="prefs-body">
      <div class="prefs-nav" role="tablist" aria-orientation="vertical">
        <h2 id="prefs-title">{{ t("prefs.title") }}</h2>
        <button
          v-for="c in categories"
          :key="c"
          type="button"
          class="nav"
          role="tab"
          :class="{ active: category === c }"
          :aria-selected="category === c"
          @click="category = c"
        >
          {{ t(`prefs.${c}` as MsgKey) }}
        </button>
      </div>

      <div class="prefs-pane" role="tabpanel">
        <template v-if="category === 'interface'">
          <div class="form">
            <span>{{ t("prefs.language") }}</span>
            <RSelect v-model="prefs.lang" :options="languages" />
            <span>{{ t("prefs.scheme") }}</span>
            <RSelect v-model="prefs.theme" :options="themes" />
          </div>
          <label class="check"><input v-model="prefs.compact" type="checkbox" /> {{ t("prefs.compact") }}</label>
          <label class="check"><input type="checkbox" :checked="contextMenu" @change="toggleContextMenu" /> {{ t("prefs.contextMenu") }}</label>
        </template>

        <template v-else-if="category === 'list'">
          <label class="check"><input v-model="prefs.mark32" type="checkbox" /> {{ t("prefs.mark32") }}</label>
          <label class="check"><input v-model="prefs.highlightRecent" type="checkbox" /> {{ t("prefs.highlightRecent") }}</label>
          <label class="check"><input v-model="prefs.showUpdates" type="checkbox" /> {{ t("prefs.showUpdates") }}</label>
          <div class="form">
            <span>{{ t("prefs.updates") }}</span>
            <RSelect v-model="prefs.updates" :options="updates" :disabled="!prefs.showUpdates" />
          </div>
        </template>

        <template v-else-if="category === 'update'">
          <label class="check"><input v-model="prefs.checkUpdates" type="checkbox" /> {{ t("prefs.checkUpdates") }}</label>
          <label class="check"><input v-model="prefs.autoUpdate" type="checkbox" :disabled="!prefs.checkUpdates" /> {{ t("prefs.autoUpdate") }}</label>
        </template>

        <template v-else>
          <label class="check"><input v-model="prefs.confirmUninstall" type="checkbox" /> {{ t("prefs.confirmUninstall") }}</label>
          <label class="check"><input v-model="prefs.quietUninstall" type="checkbox" /> {{ t("prefs.quietUninstall") }}</label>
          <div class="form">
            <span>{{ t("prefs.wizard") }}</span>
            <RSelect v-model="prefs.wizard" :options="wizard" />
            <span>{{ t("prefs.quarantine") }}</span>
            <RSelect v-model="prefs.quarantine" :options="retention" :disabled="prefs.permanentDelete" />
          </div>
          <label class="check"><input v-model="prefs.permanentDelete" type="checkbox" /> {{ t("prefs.permanentDelete") }}</label>
        </template>

        <span class="spacer"></span>
        <form method="dialog" class="dialog-buttons">
          <span class="spacer"></span>
          <button class="btn primary" autofocus>{{ t("btn.ok") }}</button>
        </form>
      </div>
    </div>
  </dialog>
</template>
