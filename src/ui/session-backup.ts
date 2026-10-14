import { isRemoteRuntime } from "../runtime";
import { shellInvoke } from "../shell";

import type { Backup } from "../engine/actions-sessions";

/**
 * Saves a backup as `drift-<name>-<date>.json`: the desktop asks where, a browser downloads it. Returns where it
 * went (a path, or the browser's downloads), or nothing when the save dialog was closed.
 */
export async function saveBackup(backup: Backup, name: string): Promise<string | undefined> {
    const slug =
        name
            .replace(/[^\p{L}\p{N}]+/gu, "-")
            .replace(/^-|-$/g, "")
            .slice(0, 60) || "sessions";
    const file = `drift-${slug}-${new Date().toISOString().slice(0, 10)}.json`;
    const contents = JSON.stringify(backup);

    const invoke = shellInvoke();
    if (invoke && !isRemoteRuntime())
        return (await invoke<string | null>("save_json_file", { name: file, contents })) ?? undefined;

    const url = URL.createObjectURL(new Blob([contents], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = file;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);

    return file;
}

/** Asks for a backup file; nothing when the picker is closed. A file that is not JSON throws. */
export function pickBackup(): Promise<Backup | undefined> {
    return new Promise((resolve, reject) => {
        const input = document.createElement("input");
        input.type = "file";
        input.accept = ".json,application/json";
        input.onchange = () => {
            const file = input.files?.[0];
            if (!file) return resolve(undefined);
            file.text()
                .then((text) => resolve(JSON.parse(text) as Backup))
                .catch(reject);
        };
        input.oncancel = () => resolve(undefined);
        input.click();
    });
}
