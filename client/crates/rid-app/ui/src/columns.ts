import { computed, reactive } from "vue";
import { prefs } from "./prefs";

export interface ColumnDef {
  key: string;
  label: string;
  /** Starting width in pixels; 0 takes the room the other columns leave. */
  width: number;
  sortable?: boolean;
  /** Switched off in the View menu. */
  hidden?: boolean;
}

const MIN = 48;
/** What a column without a width of its own is given before the list scrolls sideways instead. */
const FLEXIBLE = 220;

/** The columns of one table in the user's order and widths, which are remembered under `table`. */
export function useColumns(table: string, defs: () => ColumnDef[]) {
  const saved = () => (prefs.tables[table] ??= { order: [], widths: {} });

  const ordered = computed(() => {
    const order = prefs.tables[table]?.order ?? [];
    // Columns the saved order does not know (added in a later version) keep their default place.
    const place = (d: ColumnDef, i: number) => (order.includes(d.key) ? order.indexOf(d.key) : order.length + i);
    return defs()
      .map((d, i) => ({ d, at: place(d, i) }))
      .sort((a, b) => a.at - b.at)
      .map(({ d }) => d);
  });
  const columns = computed(() =>
    ordered.value.filter((d) => !d.hidden).map((d) => ({ ...d, width: prefs.tables[table]?.widths[d.key] ?? d.width })),
  );
  const drag = reactive({ key: "", over: "", after: false });

  function resize(e: PointerEvent, key: string) {
    const handle = e.currentTarget as HTMLElement;
    const from = handle.parentElement!.offsetWidth - e.clientX;
    handle.setPointerCapture(e.pointerId);
    handle.onpointermove = (move) => (saved().widths[key] = Math.max(MIN, Math.round(from + move.clientX)));
    handle.onpointerup = () => (handle.onpointermove = handle.onpointerup = null);
  }

  /** Dragging a heading sideways moves its column; a plain click still reaches the heading. */
  function grab(e: PointerEvent, key: string) {
    if (e.button !== 0) return;
    const heading = e.currentTarget as HTMLElement;
    const row = heading.closest("tr")!;
    const from = e.clientX;
    heading.setPointerCapture(e.pointerId);
    heading.onpointermove = (move) => {
      if (!drag.key && Math.abs(move.clientX - from) < 6) return;
      drag.key = key;
      for (const th of row.querySelectorAll<HTMLElement>("th[data-key]")) {
        const box = th.getBoundingClientRect();
        if (move.clientX < box.left || move.clientX > box.right) continue;
        drag.over = th.dataset.key!;
        drag.after = move.clientX > box.left + box.width / 2;
      }
    };
    heading.onpointerup = () => {
      heading.onpointermove = heading.onpointerup = null;
      if (!drag.key) return;
      // The release would otherwise also count as a click on the heading and sort by it.
      heading.addEventListener("click", (click) => click.stopImmediatePropagation(), { capture: true, once: true });
      if (drag.over && drag.over !== key) {
        const keys = ordered.value.map((d) => d.key).filter((k) => k !== key);
        keys.splice(keys.indexOf(drag.over) + (drag.after ? 1 : 0), 0, key);
        saved().order = keys;
      }
      drag.key = drag.over = "";
    };
  }

  return reactive({
    columns,
    minWidth: computed(() => columns.value.reduce((sum, c) => sum + (c.width || FLEXIBLE), 0)),
    /** With every width fixed, an empty last column takes up what is left of the list's width. */
    filler: computed(() => columns.value.every((c) => c.width > 0)),
    drag,
    resize,
    grab,
    reset: (key: string) => delete saved().widths[key],
  });
}

export type Columns = ReturnType<typeof useColumns>;
