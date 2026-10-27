import type { components } from "./native/types";

export type ModelInfo = components["schemas"]["Model"];
export type ProviderInfo = Pick<components["schemas"]["ProviderStatus"], "id" | "name" | "models" | "family">;
export type ProviderFamily = components["schemas"]["ProviderFamily"];
export type ProviderAccount = components["schemas"]["ProviderAccount"];

export function variantNames(model: ModelInfo | undefined) {
    return (model?.variants ?? []).map((variant) => variant.name);
}
