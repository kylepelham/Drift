import type { SetStoreFunction } from "solid-js/store"
import type { EngineState, ModelInfo, ProviderInfo } from "../engine/store"
import { persisted } from "./persist"

/**
 * The last provider catalog the engine reported, persisted across restarts.
 *
 * Cold engine startup can take seconds, during which the model picker is empty and the saved
 * model preference cannot resolve (it renders as "Default"). Seeding the last known catalog
 * keeps the picker populated instantly; the first hydrate overwrites it with fresh data.
 */
export type ProviderCatalog = {
  providers: ProviderInfo[]
  connected: string[]
  defaultModels: Record<string, string>
}

/**
 * A full ProviderInfo carries the entire engine Model shape (api endpoint, cost tables, options,
 * headers) for every model of every provider, which for a catalog with openrouter-sized model
 * lists runs to hundreds of kilobytes. The cache therefore strips each model to the fields the
 * UI actually reads: id/name (picker rows), capabilities (toolcall filter, attachment checks),
 * limit (context meter, LM Studio readiness), and family/release_date/variants (picker grouping
 * and the reasoning-variant menu). The stripped object is structurally partial but covers every
 * read Drift performs, and fresh engine data replaces it wholesale on hydrate.
 */
function compactModel(model: ModelInfo): ModelInfo {
  return {
    id: model.id,
    name: model.name,
    capabilities: model.capabilities,
    limit: model.limit,
    ...(model.family !== undefined ? { family: model.family } : {}),
    ...(model.release_date !== undefined ? { release_date: model.release_date } : {}),
    ...(model.variants !== undefined ? { variants: model.variants } : {}),
  } as ModelInfo
}

function compactCatalog(catalog: ProviderCatalog): ProviderCatalog {
  return {
    providers: catalog.providers.map((provider) => ({
      id: provider.id,
      name: provider.name,
      models: Object.fromEntries(Object.entries(provider.models).map(([key, model]) => [key, compactModel(model)])),
    })),
    connected: catalog.connected,
    defaultModels: catalog.defaultModels,
  }
}

/** Validates a stored catalog; anything malformed is dropped so a corrupt cache seeds nothing. */
export function normalizeProviderCatalog(value: unknown): ProviderCatalog | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  if (!Array.isArray(record.providers)) return null
  const providers: ProviderInfo[] = []
  for (const entry of record.providers) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue
    const provider = entry as Record<string, unknown>
    if (typeof provider.id !== "string" || typeof provider.name !== "string") continue
    if (!provider.models || typeof provider.models !== "object" || Array.isArray(provider.models)) continue
    const models: Record<string, ModelInfo> = {}
    for (const [key, candidate] of Object.entries(provider.models as Record<string, unknown>)) {
      if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) continue
      const model = candidate as Record<string, unknown>
      if (typeof model.id !== "string" || typeof model.name !== "string") continue
      if (!model.capabilities || typeof model.capabilities !== "object") continue
      if (!model.limit || typeof model.limit !== "object") continue
      models[key] = compactModel(model as unknown as ModelInfo)
    }
    if (Object.keys(models).length) providers.push({ id: provider.id, name: provider.name, models })
  }
  if (!providers.length) return null
  const connected = Array.isArray(record.connected)
    ? record.connected.filter((id): id is string => typeof id === "string")
    : []
  const defaultModels: Record<string, string> = {}
  if (record.defaultModels && typeof record.defaultModels === "object" && !Array.isArray(record.defaultModels))
    for (const [key, model] of Object.entries(record.defaultModels as Record<string, unknown>))
      if (typeof model === "string") defaultModels[key] = model
  return { providers, connected, defaultModels }
}

// persisted() guards a missing/failing localStorage (remote browser runtime), leaving the
// catalog null so seeding is a no-op there - same behaviour as the other persisted state.
const [catalog, setCatalog] = persisted<ProviderCatalog | null>(
  "drift.providers.cache",
  null,
  normalizeProviderCatalog,
)

export function cachedProviderCatalog() {
  return catalog()
}

/** Records a fresh engine catalog, skipping the write when nothing the UI reads changed. */
export function rememberProviderCatalog(
  providers: ProviderInfo[],
  connected: string[],
  defaultModels: Record<string, string>,
) {
  const next = compactCatalog({ providers, connected, defaultModels })
  const current = catalog()
  if (current && JSON.stringify(current) === JSON.stringify(next)) return
  if (!next.providers.length && !current) return
  setCatalog(next.providers.length ? next : null)
}

/**
 * Seeds engine state from the last known catalog before the first hydrate completes.
 *
 * Runs once at EngineProvider startup; the guard keeps a late call from clobbering fresh engine
 * data, and hydrate/refreshProviders always overwrite unconditionally, so a stale cache can never
 * mask reality. If a cached provider has since disappeared, resolveModel's fallback copes.
 */
export function seedProviderCatalog(state: EngineState, set: SetStoreFunction<EngineState>) {
  const cached = catalog()
  if (!cached || state.providers.length) return false
  set("providers", cached.providers)
  set("connected", cached.connected)
  set("defaultModels", cached.defaultModels)
  return true
}
