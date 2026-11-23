<script setup lang="ts">
import type { Columns } from "./columns";

defineProps<{ cols: Columns; sort?: { key: string; desc: boolean } }>();
defineEmits<{ sort: [key: string] }>();
</script>

<template>
  <colgroup>
    <col v-for="c in cols.columns" :key="c.key" :style="{ width: c.width ? `${c.width}px` : '' }" />
    <col v-if="cols.filler" />
  </colgroup>
  <thead>
    <tr>
      <th
        v-for="c in cols.columns"
        :key="c.key"
        :data-key="c.key"
        scope="col"
        :class="{
          dragging: cols.drag.key === c.key,
          'drop-before': cols.drag.key && cols.drag.key !== c.key && cols.drag.over === c.key && !cols.drag.after,
          'drop-after': cols.drag.key && cols.drag.key !== c.key && cols.drag.over === c.key && cols.drag.after,
        }"
        :aria-sort="sort?.key === c.key ? (sort.desc ? 'descending' : 'ascending') : undefined"
      >
        <button type="button" @pointerdown="cols.grab($event, c.key)" @click="c.sortable && $emit('sort', c.key)">
          {{ c.label }}
          <svg v-if="sort?.key === c.key" class="sort" :class="{ desc: sort.desc }" viewBox="0 0 10 10" aria-hidden="true"><path d="M2 6.5l3-3 3 3" /></svg>
        </button>
        <span class="col-resize" aria-hidden="true" @pointerdown.stop.prevent="cols.resize($event, c.key)" @dblclick="cols.reset(c.key)"></span>
      </th>
      <th v-if="cols.filler" aria-hidden="true"></th>
    </tr>
  </thead>
</template>
