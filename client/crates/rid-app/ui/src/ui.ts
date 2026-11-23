import { ref } from "vue";
import type { MenuItem } from "./MenuList.vue";

export interface Prompt {
  text: string;
  confirm: boolean;
  resolve: (ok: boolean) => void;
}

/** The message or question currently on screen; rendered by PromptDialog. */
export const prompt = ref<Prompt | null>(null);

let last: Promise<unknown> = Promise.resolve();

/** Prompts are shown one after another, never on top of each other. */
function show(text: string, confirm: boolean): Promise<boolean> {
  const shown = last.then(
    () =>
      new Promise<boolean>((resolve) => {
        prompt.value = {
          text,
          confirm,
          resolve: (ok) => {
            prompt.value = null;
            resolve(ok);
          },
        };
      }),
  );
  last = shown;
  return shown;
}

export const notify = (text: string) => show(text, false);
export const confirmBox = (text: string) => show(text, true);

/** The context menu on screen. `host` is the open dialog it belongs to: nothing outside a modal dialog can be clicked. */
export const menu = ref<{ x: number; y: number; items: MenuItem[]; host: Element } | null>(null);

export function showMenu(e: MouseEvent, items: MenuItem[]) {
  const height = items.reduce((sum, item) => sum + (item.sep ? 9 : 28), 8);
  menu.value = {
    x: Math.max(4, Math.min(e.clientX, innerWidth - 232)),
    y: Math.max(4, Math.min(e.clientY, innerHeight - height - 4)),
    items,
    host: (e.target as Element).closest("dialog") ?? document.body,
  };
}
