import { t } from "../state/i18n";

import type { CustomSound } from "../state/prefs";

// Vite replaces the call, never a check for the function, so try it; Bun's test runner throws.
function soundUrls(): Record<string, string> {
    try {
        return import.meta.glob<string>("../assets/audio/*.aac", {
            eager: true,
            query: "?no-inline",
            import: "default",
        });
    } catch {
        return {};
    }
}
const sources = soundUrls();
const soundSources = Object.fromEntries(
    Object.entries(sources).map(([path, url]) => [path.split("/").pop()?.replace(".aac", ""), url]),
);

const soundGroups = [
    ["drift.sound.group.alerts", "alert", 10],
    ["drift.sound.group.information", "bip-bop", 10],
    ["drift.sound.group.mechanical", "staplebops", 7],
    ["drift.sound.group.failure", "nope", 12],
    ["drift.sound.group.success", "yup", 6],
] as const;

export const soundOptions = soundGroups.flatMap(([group, prefix, count]) =>
    Array.from({ length: count }, (_, index) => ({
        id: `${prefix}-${String(index + 1).padStart(2, "0")}`,
        get label() {
            return t(`sound.option.${this.id.replaceAll("-", "")}`);
        },
        get group() {
            return t(group);
        },
    })),
);

export async function playAlertSound(id: string, custom: CustomSound | null) {
    if (id === "none") return;
    const src = id === "custom" ? custom?.dataUrl : soundSource(id);
    if (!src || typeof Audio === "undefined") return;
    await new Audio(src).play().catch(() => undefined);
}

function soundSource(id: string) {
    return soundSources[id];
}
