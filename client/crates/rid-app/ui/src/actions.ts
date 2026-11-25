import { computed } from "vue";
import { type App, api, appIcon } from "./api";
import { t } from "./i18n";
import type { MenuItem } from "./MenuList.vue";
import { current, refresh, selection, start, tracedView, tracing, upgradeJob, upgradeOf, upgradePackages } from "./store";
import { asking, confirmBox, notify } from "./ui";

export const canUninstall = (a: App) => a.category === "store" || a.uninstall_string !== "";

/** Backend failures are shown, never swallowed: a jump that silently does nothing looks like a hang. */
export function report(e: unknown) {
  notify(String(e));
}

export const jump = {
  registry: () => current.value?.reg_path && api.openRegistry(current.value.reg_path).catch(report),
  folder: () => current.value?.install_location && api.openFolder(current.value.install_location).catch(report),
  website: () => current.value?.url && api.openUrl(current.value.url).catch(report),
  google: () =>
    current.value &&
    api.openUrl(`https://www.google.com/search?q=${encodeURIComponent(current.value.name)}`).catch(report),
};

export const askAbout = (a: App) => (asking.value = { title: a.name, icon: appIcon(a), question: { feature: "program", id: a.id } });

export const actionItems = computed<MenuItem[]>(() => {
  const sel = selection.value;
  const one = current.value;
  const many = sel.length > 1;
  const registryOnly = sel.length > 0 && sel.every((a) => a.reg_path !== "");
  const traced = one?.traced ? one : null;
  return [
    {
      label: t("act.uninstall"),
      key: "Enter",
      disabled: !sel.length || !sel.every(canUninstall),
      run: () => start("uninstall"),
    },
    { label: t("act.modify"), disabled: !one?.modify_path, run: () => start("modify") },
    {
      label: one?.update ? t("act.updateTo", one.update) : t("act.update"),
      disabled: !one?.update || upgradeJob.running,
      run: () => {
        const update = one && upgradeOf.get(one.id);
        if (!update) return;
        upgradePackages([update])
          .then(() => {
            const failed = upgradeJob.failures;
            if (failed.length) report(failed.map((f) => `${f.name}: ${f.message}`).join("\n"));
          })
          .catch(report);
      },
    },
    {
      label: t(many ? "act.forceBatch" : "act.force"),
      key: "Shift+Del",
      disabled: !sel.length,
      run: () => start("force"),
    },
    {
      label: t(many ? "act.removeEntryBatch" : "act.removeEntry"),
      disabled: !registryOnly,
      run: () => start("entry"),
    },
    { sep: true },
    { label: t("act.askAi"), disabled: !one, run: () => one && askAbout(one) },
    {
      label: t("act.traced"),
      disabled: !traced,
      children: [
        {
          label: t("act.tracedView"),
          run: () => traced && api.traceGet(traced.id).then((r) => (tracedView.value = r), report),
        },
        {
          label: t("act.tracedForget"),
          run: async () => {
            if (!traced || !(await confirmBox(t("tr.forgetConfirm", traced.name)))) return;
            api.traceForget(traced.id).then(() => refresh(), report);
          },
        },
        { label: t("act.tracedAppend"), run: () => traced && (tracing.value = { appendTo: traced }) },
      ],
    },
    { label: t("act.registry"), key: "Ctrl+R", disabled: !one?.reg_path, run: jump.registry },
    { label: t("act.folder"), key: "Ctrl+E", disabled: !one?.install_location, run: jump.folder },
    { label: t("act.website"), key: "Ctrl+W", disabled: !one?.url, run: jump.website },
    {
      label: one ? `${t("act.google")} "${one.name}"` : t("act.google"),
      key: "Ctrl+G",
      disabled: !one,
      run: jump.google,
    },
  ];
});
