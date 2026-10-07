import { createSignal } from "solid-js"
import type { components } from "../engine/native/types"

export type RegistrySource = components["schemas"]["RegistrySource"]
export type RegistryKind = RegistrySource["kind"]

type Client = { settings(): Promise<{ registrySources?: RegistrySource[] | null }>; putSettings(body: { registrySources: RegistrySource[] }): Promise<unknown> }

const [sources, setSources] = createSignal<RegistrySource[]>([])
const [loaded, setLoaded] = createSignal(false)
export { sources as registrySources }

/** The user's registries from the engine, read once and kept in step with every save. */
export async function loadRegistrySources(client: Client) {
  if (loaded()) return sources()
  const settings = await client.settings()
  setSources(settings.registrySources ?? [])
  setLoaded(true)
  return sources()
}

export async function saveRegistrySources(client: Client, next: RegistrySource[]) {
  await client.putSettings({ registrySources: next })
  setSources(next)
  setLoaded(true)
}

export const sourcesOf = (kind: RegistryKind) => sources().filter((source) => source.kind === kind)

/** A URL is a registry only over https; everything else is refused before it is saved. */
export function validSourceUrl(url: string) {
  try {
    return new URL(url).protocol === "https:"
  } catch {
    return false
  }
}
