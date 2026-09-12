import type { Plugin, PluginOptions } from "@opencode-ai/plugin"

type Family = { id: string; original: string; default: string; variants?: Family[] }
type Catalog = { version: number; families: Family[] }
type Settings = { version: number; families: Record<string, string> }
type Options = { catalogPath: string; settingsPath: string }

export const PromptOverrides: Plugin = async (_input, options) => {
  const paths = readOptions(options)
  const catalog = paths ? await readJson<Catalog>(paths.catalogPath) : undefined
  const settings = paths ? await readJson<Settings>(paths.settingsPath) : undefined

  return {
    async "experimental.chat.system.transform"(input, output) {
      const family = catalog?.families.find((item) => item.id === familyFor(input.model.api.id))
      // GPT-6 changes the upstream template, not the persisted GPT/Codex override key.
      const template = (family?.id === "gpt" || family?.id === "codex") && input.model.api.id.includes("gpt-6")
        ? family?.variants?.find((item) => item.id === "gpt-astra")
        : family
      const system = output.system[0]
      if (!family || !template || !system?.startsWith(template.original)) return
      const replacement = settings?.families[family.id] ?? template.default
      output.system[0] = compatibleIdentity(family.id, replacement, template.default) + system.slice(template.original.length)
    },
  }
}

function compatibleIdentity(family: string, prompt: string, defaultPrompt: string) {
  if (family !== "anthropic") return prompt
  const identity = "You are Drift. You are OpenCode under the hood and present yourself to the user as Drift."
  if (prompt.startsWith(identity)) return prompt
  const boundary = prompt.indexOf("\n\n")
  const remainder = boundary >= 0 && prompt === defaultPrompt ? prompt.slice(boundary + 2) : prompt
  return `${identity}\n\n${remainder}`
}

function familyFor(id: string) {
  if (id.includes("muse-spark")) return "meta"
  if (id.includes("gpt-4") || id.includes("o1") || id.includes("o3")) return "beast"
  if (id.includes("gpt")) return id.includes("codex") ? "codex" : "gpt"
  if (id.includes("gemini-")) return "gemini"
  if (id.includes("claude")) return "anthropic"
  if (id.toLowerCase().includes("trinity")) return "trinity"
  if (id.toLowerCase().includes("kimi")) return "kimi"
  return "default"
}

function readOptions(value?: PluginOptions): Options | undefined {
  if (!value || typeof value.catalogPath !== "string" || typeof value.settingsPath !== "string") return
  return { catalogPath: value.catalogPath, settingsPath: value.settingsPath }
}

async function readJson<T>(file: string): Promise<T | undefined> {
  return Bun.file(file)
    .json()
    .catch(() => undefined) as Promise<T | undefined>
}
