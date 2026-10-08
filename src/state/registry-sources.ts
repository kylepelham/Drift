import { createSignal } from "solid-js";

import type { components } from "../engine/native/types";

export type RegistrySource = components["schemas"]["RegistrySource"];
export type SourceInput = components["schemas"]["SourceInput"];
export type RegistryKind = RegistrySource["kind"];
export type SourceKind = NonNullable<RegistrySource["source"]>;

export type SourcesClient = {
    settings(): Promise<{ registrySources?: RegistrySource[] | null }>;
    putSettings(body: { registrySources: SourceInput[] }): Promise<unknown>;
};

const [sources, setSources] = createSignal<RegistrySource[]>([]);
const [loaded, setLoaded] = createSignal(false);
export { sources as registrySources };

/** The user's registries from the engine, read once and kept in step with every save. */
export async function loadRegistrySources(client: SourcesClient) {
    if (loaded()) return sources();
    const settings = await client.settings();
    setSources(settings.registrySources ?? []);
    setLoaded(true);
    return sources();
}

/** Saves the whole list; `tokens` carries a new token for a source by id (empty clears it). */
export async function saveRegistrySources(
    client: SourcesClient,
    next: RegistrySource[],
    tokens: Record<string, string> = {},
) {
    await client.putSettings({
        registrySources: next.map((source) => (source.id in tokens ? { ...source, token: tokens[source.id] } : source)),
    });
    const settings = await client.settings();
    setSources(settings.registrySources ?? next);
    setLoaded(true);
}

export const sourcesOf = (kind: RegistryKind) => sources().filter((source) => source.kind === kind);

/** What the location field must look like for a source kind, before it is saved. */
export function sourceProblem(kind: SourceKind, url: string, allowHttp: boolean): string | undefined {
    const value = url.trim();
    if (!value) return "empty";
    if (kind === "url") {
        try {
            const protocol = new URL(value).protocol;
            if (protocol === "https:") return undefined;
            if (protocol === "http:") return allowHttp ? undefined : "http";
            return "url";
        } catch {
            return "url";
        }
    }
    if (kind === "github") return /^https:\/\/github\.com\/[^/]+\/[^/]+/.test(value) ? undefined : "github";
    if (kind === "azure_devops")
        return /^https:\/\/(dev\.azure\.com\/[^/]+\/[^/]+|[^/]+\.visualstudio\.com\/[^/]+)\/_git\/[^/]+/.test(value)
            ? undefined
            : "azure";
    return undefined;
}

export const validSourceUrl = (url: string) => sourceProblem("url", url, false) === undefined;
