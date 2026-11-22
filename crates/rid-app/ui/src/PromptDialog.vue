<script setup lang="ts">
import { onMounted, ref } from "vue";
import { t } from "./i18n";
import type { Prompt } from "./ui";

const props = defineProps<{ prompt: Prompt }>();
const dialog = ref<HTMLDialogElement>();
onMounted(() => dialog.value?.showModal());
</script>

<template>
  <!-- Esc or any other way of closing counts as "no". -->
  <dialog ref="dialog" class="dialog prompt" role="alertdialog" aria-describedby="prompt-text" @close="props.prompt.resolve(false)">
    <p id="prompt-text">{{ prompt.text }}</p>
    <div class="dialog-buttons">
      <button v-if="prompt.confirm" type="button" class="btn" @click="prompt.resolve(false)">{{ t("btn.cancel") }}</button>
      <button type="button" class="btn primary" autofocus @click="prompt.resolve(true)">{{ t("btn.ok") }}</button>
    </div>
  </dialog>
</template>
