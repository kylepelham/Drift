import type { components } from "../engine/native/types";

/** The registry as Drift-Plugins publishes it. */
export type RegistryPlugin = {
    /** The registry it came from; unset for Drift's own. */
    sourceName?: string;
    sourceId?: string;
    /** A WebAssembly component (the default), one Markdown skill, or a pack of them, unpacked from an archive. */
    kind?: "wasm" | "skill" | "skills";
    /** For a skill or pack: the tar.gz to fetch, the folders inside it to keep, and the skills it holds. */
    archive?: string;
    subdirs?: string[];
    skills?: { name: string; description: string }[];
    id: string;
    name: string;
    description: string;
    category: string;
    /** Its picture, an https image URL; the tile shows its initial without one. */
    image?: string;
    hooks: string[];
    config: ConfigField[];
    version: string;
    author: string;
    source: string;
    download: string;
    sha256: string;
    size: number;
};

export type ConfigField = {
    key: string;
    label: string;
    type: "string" | "list" | "boolean" | "number" | "json";
    default?: unknown;
    description?: string;
};

export type Registry = { version: number; plugins: RegistryPlugin[] };

export const registryUrl = "https://raw.githubusercontent.com/kylepelham/Drift-Plugins/main/registry.json";
export const registryCategories = ["safety", "quality", "workflow", "context", "notify"] as const;

const CACHE_MS = 10 * 60 * 1000;
const cached = new Map<string, { at: number; registry: Registry }>();

/** One registry document, fetched once per ten minutes; `fresh` fetches again. */
export async function loadRegistry(url = registryUrl, fresh = false): Promise<Registry> {
    const hit = cached.get(url);
    if (!fresh && hit && Date.now() - hit.at < CACHE_MS) return hit.registry;
    const response = await fetch(url, { cache: "no-store" });
    if (!response.ok) throw new Error(`registry ${response.status}`);
    const registry = (await response.json()) as Registry;
    if (!Array.isArray(registry.plugins)) throw new Error("registry has no plugins");
    cached.set(url, { at: Date.now(), registry });
    return registry;
}

export type RegistryFailure = { name: string; error: string };

/** A user's source is read by the engine, which holds its token and trust settings; the result is checked like Drift's own. */
async function loadSourceRegistry(
    fetchRegistry: (id: string) => Promise<unknown>,
    source: { id: string },
    fresh: boolean,
): Promise<Registry> {
    const key = `source:${source.id}`;
    const hit = cached.get(key);
    if (!fresh && hit && Date.now() - hit.at < CACHE_MS) return hit.registry;
    const registry = (await fetchRegistry(source.id)) as Registry;
    if (!Array.isArray(registry?.plugins)) throw new Error("registry has no plugins");
    cached.set(key, { at: Date.now(), registry });
    return registry;
}

/** Drift's registry and the user's own, the user's first; a source that fails is named, the rest still show. */
export async function loadRegistries(
    sources: { id: string; name: string }[],
    fresh = false,
    fetchRegistry?: (id: string) => Promise<unknown>,
): Promise<{ plugins: RegistryPlugin[]; failures: RegistryFailure[] }> {
    const plugins: RegistryPlugin[] = [];
    const failures: RegistryFailure[] = [];
    const viaEngine = fetchRegistry ?? (() => Promise.reject(new Error("no engine")));
    const results = await Promise.allSettled([
        ...sources.map((source) => loadSourceRegistry(viaEngine, source, fresh)),
        loadRegistry(registryUrl, fresh),
    ]);
    results.forEach((result, index) => {
        const source = sources[index];
        if (result.status === "rejected") {
            failures.push({
                name: source?.name ?? "Drift",
                error: result.reason instanceof Error ? result.reason.message : String(result.reason),
            });
            return;
        }
        for (const plugin of result.value.plugins)
            plugins.push(source ? { ...plugin, sourceName: source.name, sourceId: source.id } : plugin);
    });
    const seen = new Set<string>();
    return { plugins: plugins.filter((plugin) => !seen.has(plugin.id) && seen.add(plugin.id)), failures };
}

/** The drift.json entry an installed registry plugin has. */
export const installedPath = (id: string) => `plugins/${id}.wasm`;

export const isSkillEntry = (plugin: Pick<RegistryPlugin, "kind">) =>
    plugin.kind === "skill" || plugin.kind === "skills";

/** Whether a plugin matches a search: name, description, category, hooks. */
export function matchesRegistryQuery(plugin: RegistryPlugin, query: string) {
    const needle = query.trim().toLowerCase();
    if (!needle) return true;
    return [plugin.name, plugin.id, plugin.description, plugin.category, plugin.sourceName ?? "", ...plugin.hooks].some(
        (text) => text.toLowerCase().includes(needle),
    );
}

/** A field's typed value from what was typed, or its default when nothing was. */
export function fieldValue(field: ConfigField, typed: string | undefined): unknown {
    if (typed === undefined) return field.default;
    if (field.type === "boolean") return typed === "true";
    if (field.type === "number") {
        const number = Number(typed);
        return typed.trim() === "" || Number.isNaN(number) ? field.default : number;
    }
    if (field.type === "list") return typed.trim() === "" ? [] : typed.split(/\s*,\s*/).filter(Boolean);
    if (field.type === "json") {
        try {
            return JSON.parse(typed);
        } catch {
            return field.default;
        }
    }
    return typed;
}

/** A field's value as text for its input. */
export function fieldText(field: ConfigField, value: unknown): string {
    if (value === undefined || value === null) return "";
    if (field.type === "list") return Array.isArray(value) ? value.join(", ") : String(value);
    if (field.type === "json") return JSON.stringify(value, null, 2);
    return String(value);
}

/** The config object to store: every field, from the values typed or the defaults. */
export function buildConfig(fields: ConfigField[], typed: Record<string, string>) {
    const config: Record<string, unknown> = {};
    for (const field of fields) {
        const value = fieldValue(field, typed[field.key]);
        if (value !== undefined) config[field.key] = value;
    }
    return config;
}

export type PluginInfo = components["schemas"]["PluginInfo"];
