// A plugin's declared settings as a form draft, and the draft back as the config object Drift saves.
import type { components } from "../engine/native/types";

export type PluginSetting = components["schemas"]["Setting"];
export type PluginSettingKind = PluginSetting["kind"];
export type PairRow = { key: string; value: string };
/** Text for text, choice and number; a flag for a toggle; rows for lists and pairs. */
export type DraftValue = string | boolean | string[] | PairRow[];
export type Draft = Record<string, DraftValue>;

const isObject = (value: unknown): value is Record<string, unknown> =>
    !!value && typeof value === "object" && !Array.isArray(value);

function textOf(value: unknown, fallback: string) {
    return typeof value === "string" ? value : fallback;
}

function draftValue(kind: PluginSettingKind, stored: unknown): DraftValue {
    switch (kind.type) {
        case "text":
        case "choice":
            return textOf(stored, kind.default);
        case "number": {
            const value = typeof stored === "number" ? stored : kind.default;
            return value === null || value === undefined ? "" : String(value);
        }
        case "toggle":
            return typeof stored === "boolean" ? stored : kind.default;
        case "items":
            return Array.isArray(stored) ? stored.map(String) : [...kind.default];
        case "pairs":
            return isObject(stored)
                ? Object.entries(stored).map(([key, value]) => ({ key, value: String(value) }))
                : kind.default.map((pair) => ({ ...pair }));
    }
}

/** What the form shows: each setting's stored value, or its default when none is stored. */
export function draftFrom(settings: PluginSetting[], stored: Record<string, unknown>): Draft {
    return Object.fromEntries(settings.map((setting) => [setting.key, draftValue(setting.kind, stored[setting.key])]));
}

function configValue(kind: PluginSettingKind, value: DraftValue): unknown {
    switch (kind.type) {
        case "number": {
            const text = String(value).trim();
            if (!text) return undefined;
            // Text that is not a number goes as it is, so the engine names the setting it does not fit.
            return Number.isNaN(Number(text)) ? text : Number(text);
        }
        case "items":
            return (value as string[]).filter((line) => line.trim());
        case "pairs":
            return Object.fromEntries(
                (value as PairRow[]).filter((row) => row.key.trim()).map((row) => [row.key, row.value]),
            );
        default:
            return value;
    }
}

/** The config to save: every declared setting from the draft, and whatever else was stored left as it was. */
export function configFrom(settings: PluginSetting[], draft: Draft, stored: Record<string, unknown>) {
    const config: Record<string, unknown> = { ...stored };
    for (const setting of settings) {
        const value = configValue(setting.kind, draft[setting.key]);
        if (value === undefined) delete config[setting.key];
        else config[setting.key] = value;
    }

    return config;
}
