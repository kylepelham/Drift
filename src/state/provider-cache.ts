import { persisted } from "./persist";

import type { EngineState, ModelInfo, ProviderInfo } from "../engine/store";
import type { SetStoreFunction } from "solid-js/store";

/** Persists the last provider catalog so the picker has choices before the engine hydrates. */
export type ProviderCatalog = {
    providers: ProviderInfo[];
    connected: string[];
    defaultModels: Record<string, string>;
};

/** Keeps only the model fields pickers, attachment checks, context limits and reasoning choices read, so large catalogs stay cheap to cache. */
function compactModel(model: ModelInfo): ModelInfo {
    return {
        id: model.id,
        name: model.name,
        limit: model.limit,
        attachment: model.attachment,
        pdf: model.pdf,
        reasoning: model.reasoning,
        temperature: model.temperature,
        ...(model.family !== undefined ? { family: model.family } : {}),
        ...(model.release_date !== undefined ? { release_date: model.release_date } : {}),
        ...(model.variants !== undefined ? { variants: model.variants } : {}),
    };
}

function compactCatalog(catalog: ProviderCatalog): ProviderCatalog {
    return {
        providers: catalog.providers.map((provider) => ({
            id: provider.id,
            name: provider.name,
            models: Object.fromEntries(
                Object.entries(provider.models).map(([key, model]) => [key, compactModel(model)]),
            ),
        })),
        connected: catalog.connected,
        defaultModels: catalog.defaultModels,
    };
}

/** Validates a stored catalog; anything malformed is dropped so a corrupt cache seeds nothing. */
export function normalizeProviderCatalog(value: unknown): ProviderCatalog | null {
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;

    const record = value as Record<string, unknown>;
    if (!Array.isArray(record.providers)) return null;

    const providers: ProviderInfo[] = [];
    for (const entry of record.providers) {
        const provider = normalizeProvider(entry);
        if (provider) providers.push(provider);
    }
    if (!providers.length) return null;

    const connected = Array.isArray(record.connected)
        ? record.connected.filter((id): id is string => typeof id === "string")
        : [];
    const defaultModels: Record<string, string> = {};
    if (record.defaultModels && typeof record.defaultModels === "object" && !Array.isArray(record.defaultModels))
        for (const [key, model] of Object.entries(record.defaultModels as Record<string, unknown>))
            if (typeof model === "string") defaultModels[key] = model;

    return { providers, connected, defaultModels };
}

function normalizeProvider(entry: unknown): ProviderInfo | undefined {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return;

    const provider = entry as Record<string, unknown>;
    if (typeof provider.id !== "string" || typeof provider.name !== "string") return;
    if (!provider.models || typeof provider.models !== "object" || Array.isArray(provider.models)) return;

    const models: Record<string, ModelInfo> = {};
    for (const [key, candidate] of Object.entries(provider.models)) {
        const model = normalizeModel(candidate);
        if (model) models[key] = model;
    }
    if (Object.keys(models).length) return { id: provider.id, name: provider.name, models };
}

function normalizeModel(candidate: unknown) {
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) return;

    const model = candidate as Record<string, unknown>;
    if (typeof model.id !== "string" || typeof model.name !== "string") return;

    return compactModel(restoreModel(model));
}

function restoreModel(model: Record<string, unknown>) {
    // Older caches stored SDK capabilities and keyed variants rather than native catalog fields.
    const legacy = model.capabilities as
        { input?: { image?: boolean; pdf?: boolean }; reasoning?: boolean } | undefined;
    const variants = Array.isArray(model.variants)
        ? model.variants
        : Object.entries((model.variants ?? {}) as Record<string, object>).map(([name, value]) => ({ ...value, name }));
    return {
        ...model,
        attachment: model.attachment ?? legacy?.input?.image,
        pdf: model.pdf ?? legacy?.input?.pdf,
        reasoning: model.reasoning ?? legacy?.reasoning,
        variants,
    } as ModelInfo;
}

// Failed or unavailable localStorage leaves the catalog null, so startup seeding does nothing.
const [catalog, setCatalog] = persisted<ProviderCatalog | null>(
    "drift.providers.cache",
    null,
    normalizeProviderCatalog,
);

export function cachedProviderCatalog() {
    return catalog();
}

/** Records a fresh engine catalog, skipping the write when nothing the UI reads changed. */
export function rememberProviderCatalog(
    providers: ProviderInfo[],
    connected: string[],
    defaultModels: Record<string, string>,
) {
    const next = compactCatalog({ providers, connected, defaultModels });
    const current = catalog();
    if (current && JSON.stringify(current) === JSON.stringify(next)) return;
    if (!next.providers.length && !current) return;

    setCatalog(next.providers.length ? next : null);
}

/** A provider listing as the engine reports it; `undefined` means the request produced no payload. */
export type ProviderListing = { all?: unknown; connected?: string[]; default?: Record<string, string> };

/** Applies a provider listing to state and cache; a missing payload leaves both unchanged. */
export function applyProviderCatalog(set: SetStoreFunction<EngineState>, data: ProviderListing | undefined) {
    if (data === undefined) return false;

    const providers = (data.all ?? []) as ProviderInfo[];
    const connected = data.connected ?? [];
    const defaultModels = data.default ?? {};

    set("providers", providers);
    set("connected", connected);
    set("defaultModels", defaultModels);
    rememberProviderCatalog(providers, connected, defaultModels);

    return true;
}

/** Seeds startup state from the cached catalog only when fresh providers have not arrived. */
export function seedProviderCatalog(state: EngineState, set: SetStoreFunction<EngineState>) {
    const cached = catalog();
    if (!cached || state.providers.length) return false;

    set("providers", cached.providers);
    set("connected", cached.connected);
    set("defaultModels", cached.defaultModels);

    return true;
}
