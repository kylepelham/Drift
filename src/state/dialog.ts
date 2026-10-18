import { isRemoteRuntime } from "../runtime";
import { shellInvoke } from "../shell";
import { t } from "./i18n";

export async function pickFolder(): Promise<string | null> {
    if (isRemoteRuntime()) return window.prompt(t("drift.workspace.pathPrompt.remote"));
    const invoke = shellInvoke();
    if (invoke) return (await invoke<string | null>("pick_folder")) ?? null;
    return window.prompt(t("drift.workspace.pathPrompt"));
}
