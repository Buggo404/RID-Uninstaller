import { ref } from "vue";

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
