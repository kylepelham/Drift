// Drift's one folder in the system's temporary directory: scripts and tests write only in subfolders of it.
import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

/** A subfolder of `Drift` in the temporary directory, made if it is missing. */
export function driftTemp(area: string) {
    const directory = path.join(tmpdir(), "Drift", area);
    mkdirSync(directory, { recursive: true });
    return directory;
}

/** A new folder of its own under `area`, named from `prefix`. */
export const tempFolder = (area: string, prefix: string) => mkdtempSync(path.join(driftTemp(area), prefix));
