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
  <ul class="menu" role="menu" @click.stop @contextmenu.prevent.stop>
    <template v-for="(item, i) in items" :key="i">
      <li v-if="item.sep" class="menu-sep" role="separator"></li>
      <li v-else class="menu-item" role="none">
        <!-- mousedown.prevent: the field a menu was opened on keeps the focus its commands act on. -->
        <button
          type="button"
          role="menuitem"
          :disabled="item.disabled"
          :aria-haspopup="item.children ? 'menu' : undefined"
          @mousedown.prevent
          @click="pick(item)"
        >
          <svg class="menu-check" viewBox="0 0 12 12" aria-hidden="true"><path v-if="item.checked" d="M2.5 6.5l2.5 2.5 4.5-5.5" /></svg>
          <span class="menu-label">{{ item.label }}</span>
          <svg v-if="item.children" class="menu-more" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" /></svg>
          <span v-else class="menu-key">{{ item.key }}</span>
        </button>
        <MenuList v-if="item.children && !item.disabled" class="submenu" :items="item.children" @close="emit('close')" />
      </li>
    </template>
  </ul>
</template>
