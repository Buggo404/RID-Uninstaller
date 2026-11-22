<script setup lang="ts" generic="T extends string">
import { computed, ref } from "vue";

const props = defineProps<{ options: { value: T; label: string }[]; disabled?: boolean }>();
const model = defineModel<T>({ required: true });
const open = ref(false);
const active = ref(0);
const current = computed(() => props.options.find((o) => o.value === model.value));

function toggle() {
  if (props.disabled) return;
  open.value = !open.value;
  active.value = Math.max(0, props.options.findIndex((o) => o.value === model.value));
}

function pick(value: T) {
  model.value = value;
  open.value = false;
}

function onKey(e: KeyboardEvent) {
  const opens = ["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key);
  if (!open.value) {
    if (opens) {
      e.preventDefault();
      toggle();
    }
    return;
  }
  if (e.key === "Tab") return void (open.value = false);
  if (e.key !== "Escape" && !opens) return;
  // Handled here, so that Esc closes the list and not the dialog around it.
  e.preventDefault();
  e.stopPropagation();
  const n = props.options.length;
  if (e.key === "Escape") open.value = false;
  else if (e.key === "ArrowDown") active.value = (active.value + 1) % n;
  else if (e.key === "ArrowUp") active.value = (active.value + n - 1) % n;
  else pick(props.options[active.value].value);
}

function onFocusOut(e: FocusEvent) {
  if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) open.value = false;
}
</script>

<template>
  <div class="select" :class="{ open }" @focusout="onFocusOut">
    <button
      type="button"
      class="select-trigger"
      role="combobox"
      aria-haspopup="listbox"
      :aria-expanded="open"
      :disabled="disabled"
      @click="toggle"
      @keydown="onKey"
    >
      <span>{{ current?.label }}</span>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 6.5 8 10l3.5-3.5" /></svg>
    </button>
    <Transition name="pop">
      <ul v-if="open" class="select-list" role="listbox">
        <li
          v-for="(o, i) in options"
          :key="o.value"
          role="option"
          :aria-selected="o.value === model"
          :class="{ active: i === active }"
          @mouseenter="active = i"
          @mousedown.prevent="pick(o.value)"
        >
          {{ o.label }}
        </li>
      </ul>
    </Transition>
  </div>
</template>
