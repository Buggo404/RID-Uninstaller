<script setup lang="ts">
export interface MenuItem {
  label?: string;
  /** Shortcut text shown on the right. */
  key?: string;
  run?: () => void;
  disabled?: boolean;
  checked?: boolean;
  children?: MenuItem[];
  sep?: boolean;
}

defineProps<{ items: MenuItem[] }>();
const emit = defineEmits<{ close: [] }>();

function pick(item: MenuItem) {
  if (item.children) return;
  emit("close");
  item.run?.();
}
</script>

<template>
  <ul class="menu" role="menu">
    <template v-for="(item, i) in items" :key="i">
      <li v-if="item.sep" class="menu-sep" role="separator"></li>
      <li v-else class="menu-item" role="none">
        <button
          type="button"
          role="menuitem"
          :disabled="item.disabled"
          :aria-haspopup="item.children ? 'menu' : undefined"
          @click="pick(item)"
        >
          <span class="menu-check" aria-hidden="true">{{ item.checked ? "✓" : "" }}</span>
          <span class="menu-label">{{ item.label }}</span>
          <span class="menu-key">{{ item.children ? "▸" : item.key }}</span>
        </button>
        <MenuList v-if="item.children && !item.disabled" class="submenu" :items="item.children" @close="emit('close')" />
      </li>
    </template>
  </ul>
</template>
