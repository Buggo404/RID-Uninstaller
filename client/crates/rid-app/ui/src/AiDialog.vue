<script lang="ts">
import type { Answer } from "./api";

/** Answers already given in this run; asking again about the same thing is instant and costs nothing. */
const answered = new Map<string, Answer>();
</script>

<script setup lang="ts">
import { onMounted, ref } from "vue";
import { type Question, api } from "./api";
import { lang, t } from "./i18n";

const props = defineProps<{ title: string; icon?: string; question: Question }>();
const emit = defineEmits<{ close: [] }>();

const dialog = ref<HTMLDialogElement>();
const answer = ref<Answer | null>(null);
const error = ref("");
const busy = ref(false);

async function load(fresh: boolean) {
  const key = `${lang.value}|${JSON.stringify(props.question)}`;
  const known = !fresh && answered.get(key);
  if (known) return void (answer.value = known);
  busy.value = true;
  error.value = "";
  try {
    answer.value = await api.aiAsk(props.question, lang.value);
    answered.set(key, answer.value);
  } catch (e) {
    const reason = String(e);
    error.value = reason === "ai-unavailable" ? t("ai.unavailable") : reason === "ai-busy" ? t("ai.busy") : t("ai.failed", reason);
  }
  busy.value = false;
}

onMounted(() => {
  dialog.value?.showModal();
  load(false);
});
</script>

<template>
  <dialog ref="dialog" class="dialog ai" aria-labelledby="ai-title" @close="emit('close')">
    <div class="panel-head">
      <img v-if="icon" class="app-icon" :src="icon" alt="" />
      <h2 id="ai-title">{{ title }}</h2>
    </div>

    <template v-if="busy">
      <p class="note" role="status">{{ t("ai.thinking") }}</p>
      <div class="progress" role="progressbar"><i></i></div>
    </template>
    <p v-else-if="error" class="notice" role="alert">{{ error }}</p>
    <div v-else-if="answer" class="ai-answer">
      <div class="ai-chips">
        <span class="chip" :class="answer.verdict">{{ t(`ai.${answer.verdict}`) }}</span>
        <span class="chip">{{ t(`ai.${answer.confidence}`) }}</span>
      </div>
      <p>{{ answer.summary }}</p>
      <ul v-if="answer.points.length">
        <li v-for="point in answer.points" :key="point">{{ point }}</li>
      </ul>
    </div>

    <form method="dialog" class="dialog-buttons">
      <span v-if="answer && !busy" class="note">{{ t("ai.note") }}</span>
      <span class="spacer"></span>
      <button v-if="error" type="button" class="btn" @click="load(true)">{{ t("ai.again") }}</button>
      <button class="btn primary" autofocus>{{ t("btn.close") }}</button>
    </form>
  </dialog>
</template>
