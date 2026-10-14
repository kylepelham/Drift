import type { Backup } from "../engine/actions-sessions";

/** Saves a backup as `drift-<name>-<date>.json` through the webview's own download. */
export function downloadBackup(backup: Backup, name: string) {
    const slug =
        name
            .replace(/[^\p{L}\p{N}]+/gu, "-")
            .replace(/^-|-$/g, "")
            .slice(0, 60) || "sessions";
    const date = new Date().toISOString().slice(0, 10);
    const url = URL.createObjectURL(new Blob([JSON.stringify(backup)], { type: "application/json" }));

    const link = document.createElement("a");
    link.href = url;
    link.download = `drift-${slug}-${date}.json`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
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
