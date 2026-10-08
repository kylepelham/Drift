import type { McpServerConfig } from "./engine/store"

export type RegistryInput = {
  value?: string
  default?: string
  description?: string
  isRequired?: boolean
  isSecret?: boolean
  variables?: Record<string, RegistryInput>
}
export type RegistryArgument = RegistryInput & {
  type: "named" | "positional"
  name?: string
  valueHint?: string
  isRepeated?: boolean
}
export type RegistryVariable = RegistryInput & { name: string }
export type RegistryHeader = RegistryInput & { name: string }
export type RegistryPackage = {
  transport: { type: string }
  registryType: string
  identifier: string
  version: string
  runtimeHint?: string
  runtimeArguments?: RegistryArgument[]
  packageArguments?: RegistryArgument[]
  environmentVariables?: RegistryVariable[]
}
export type RegistryRemote = {
  type: "streamable-http" | "sse"
  url: string
  headers?: RegistryHeader[]
  variables?: Record<string, RegistryInput>
}
/** How a catalog shows a server: what GitHub's registry knows of its repository, or what the entry itself says. */
export type RegistryListing = {
  source: "github" | "official" | "custom"
  /** For a custom source, the name the user gave it. */
  sourceName?: string
  publisher?: string
  image?: string
  stars?: number
  topics?: string[]
  repository?: string
  website?: string
}
export type RegistryServer = {
  name: string
  title?: string
  description: string
  version: string
  packages?: RegistryPackage[]
  remotes?: RegistryRemote[]
  listing?: RegistryListing
}

/** One thing the user may have to type: a header, a variable or a template's placeholder. */
export type InstallField = {
  key: string
  label: string
  description?: string
  secret: boolean
  required: boolean
  default?: string
}
export type InstallKind = "remote" | "npm" | "pypi" | "docker"
export type InstallOption = {
  id: string
  kind: InstallKind
  /** For a remote, the transport; for a package, the version, `latest` when the entry does not pin one. */
  detail: string
  fields: InstallField[]
  /** The engine config with the typed values in, or `null` while a required one is missing. */
  build(values: Record<string, string>): McpServerConfig | null
}

/** The engine's name for a registry server: its last path segment, kept to what a tool name may hold. */
export function registryServerName(name: string) {
  return (name.split("/").at(-1) ?? name).replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 128)
}

/** The name a server installs under: its display title when it reads as a name, so tools read `github_create_issue`. */
export function registryInstallName(server: RegistryServer) {
  const title = (server.title ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
  return title && title.length <= 32 ? title : registryServerName(server.name)
}

/** Every way Drift can run the server, remotes first; ways it cannot run are left out. */
export function registryOptions(server: RegistryServer): InstallOption[] {
  if (!validServerName(server.name)) return []
  const remotes = (server.remotes ?? []).flatMap((remote, index) => remoteOption(remote, index) ?? [])
  const packages = (server.packages ?? []).flatMap((item, index) => packageOption(item, index) ?? [])
  return [...remotes, ...packages]
}

/** The option to offer first: the first that needs nothing typed, else the first. */
export function preferredOption(options: InstallOption[]) {
  return options.find((option) => option.fields.every((field) => !field.required)) ?? options[0]
}

/** The config the registry entry gives with nothing typed, when it needs nothing typed. */
export function registryConfig(server: RegistryServer): McpServerConfig | null {
  for (const option of registryOptions(server)) {
    const config = option.build({})
    if (config) return config
  }
  return null
}

function remoteOption(remote: RegistryRemote, index: number): InstallOption | null {
  const url = template(remote.url, remote.variables, "url")
  const headers = remote.headers?.map((header) => input(header, `header:${header.name}`, header.name))
  if (!url || headers?.some((header) => !header || !validHeaderName(header.name))) return null
  const parts = headers as Part[] | undefined
  return {
    id: `remote:${index}`,
    kind: "remote",
    detail: remote.type === "sse" ? "sse" : "streamable-http",
    fields: fieldsOf([url, ...(parts ?? [])]),
    build(values) {
      const resolved = url.fill(values)
      if (resolved === null || !resolved || !validHttpsUrl(resolved)) return null
      const filled = filledPairs(parts, values, authorization)
      if (!filled) return null
      return { type: remote.type === "sse" ? "sse" : "http", url: resolved, headers: filled }
    },
  }
}

/** A key typed alone into an Authorization header is sent as a bearer token. */
function authorization(name: string, value: string) {
  return name.toLowerCase() === "authorization" && !/\s/.test(value.trim()) ? `Bearer ${value.trim()}` : value
}

const RUNTIMES: Record<string, { kind: InstallKind; command: string }> = {
  npm: { kind: "npm", command: "npx" },
  pypi: { kind: "pypi", command: "uvx" },
  oci: { kind: "docker", command: "docker" },
}

function packageOption(item: RegistryPackage, index: number): InstallOption | null {
  const runtime = RUNTIMES[item.registryType]
  if (item.transport.type !== "stdio" || !runtime || (item.runtimeHint && item.runtimeHint !== runtime.command))
    return null
  const reference = packageReference(item)
  const runtimeArguments = argumentParts(item.runtimeArguments, `runtime:${index}`)
  const packageArguments = argumentParts(item.packageArguments, `package:${index}`)
  const env = item.environmentVariables?.map((variable) => input(variable, `env:${variable.name}`, variable.name))
  if (
    !reference ||
    !runtimeArguments ||
    !packageArguments ||
    env?.some((part) => !part || !validEnvironmentName(part.name))
  )
    return null
  const envParts = (env ?? []) as Part[]
  return {
    id: `package:${index}`,
    kind: runtime.kind,
    detail: packageDetail(runtime.kind, reference, item),
    fields: fieldsOf([...runtimeArguments, ...envParts, ...packageArguments]),
    build(values) {
      const before = filledArguments(runtimeArguments, values)
      const after = filledArguments(packageArguments, values)
      const filled = filledPairs(envParts, values)
      if (!before || !after || !filled) return null
      const runtimeArgs = runtime.kind === "docker" ? dockerEnvironment(before, filled) : before
      return {
        type: "stdio",
        command: runtime.command,
        args: launch(runtime.kind, runtimeArgs, reference, after, Object.keys(filled)),
        env: filled,
      }
    },
  }
}

function packageDetail(kind: InstallKind, reference: string, item: RegistryPackage) {
  if (kind === "docker") return reference.split("@")[0].match(/:([^/:]+)$/)?.[1] ?? "latest"

  return isPinned(item.registryType, item.version) ? item.version : "latest"
}

/** npx gets `-y`, so it never stops to ask on the server's stdin; docker gets each variable passed through by name. */
function launch(kind: InstallKind, before: string[], reference: string, after: string[], env: string[]) {
  if (kind === "npm") return [...(before.includes("-y") ? [] : ["-y"]), ...before, reference, ...after]
  if (kind === "docker")
    return ["run", "-i", "--rm", ...env.flatMap((name) => ["-e", name]), ...before, reference, ...after]
  // After `uvx --from <source>` the entry names the command itself.
  if (before.includes("--from")) return [...before, ...after]
  return [...before, reference, ...after]
}

/** Docker's `-e NAME=value` moves into the server's environment, where the engine keeps it secret; docker passes it on by name. */
function dockerEnvironment(args: string[], env: Record<string, string>) {
  const kept: string[] = []
  for (let index = 0; index < args.length; index++) {
    const joined = args[index].match(/^--env=([A-Za-z_][A-Za-z0-9_]*)=(.*)$/)
    const split =
      (args[index] === "-e" || args[index] === "--env") && args[index + 1]?.match(/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/)
    const pair = joined ?? split
    if (!pair) {
      kept.push(args[index])
      continue
    }
    env[pair[1]] = pair[2]
    if (split) index++
  }
  return kept
}

function packageReference(item: RegistryPackage) {
  const pinned = isPinned(item.registryType, item.version)
  if (item.registryType === "npm")
    return validNpmIdentifier(item.identifier) ? `${item.identifier}@${pinned ? item.version : "latest"}` : null
  if (item.registryType === "pypi") {
    if (!validPypiIdentifier(item.identifier)) return null

    return pinned ? `${item.identifier}==${item.version}` : item.identifier
  }
  return validImage(item.identifier) ? item.identifier : null
}

/** A value from the entry, with the fields it leaves for the user: placeholders in a template, or the value itself when none is given. */
type Part = {
  name: string
  required: boolean
  fields: InstallField[]
  fill(values: Record<string, string>): string | null | undefined
}

function input(item: RegistryInput, key: string, name: string): Part | null {
  const given = Object.prototype.hasOwnProperty.call(item, "value") ? item.value : item.default
  const required = !!item.isRequired
  if (typeof given === "string") {
    const filled = template(given, item.variables, key, required, !!item.isSecret, item.description)
    return filled && { ...filled, name, required }
  }
  const field: InstallField = {
    key,
    label: name,
    description: item.description,
    secret: !!item.isSecret,
    required,
    default: undefined,
  }
  return { name, required, fields: [field], fill: (values) => typed(values[key]) }
}

/** A template's placeholders become fields; `fill` is `undefined` when an optional one was left empty, `null` when a required one was. */
function template(
  value: string,
  variables: Record<string, RegistryInput> | undefined,
  key: string,
  required = true,
  secret = false,
  about?: string,
): Part | null {
  if (!safeValue(value)) return null
  const names = [...new Set([...value.matchAll(/\{([A-Za-z0-9._-]+)\}/g)].map((match) => match[1]))]
  const fields: InstallField[] = []
  const fixed: Record<string, string> = {}
  for (const name of names) {
    const variable = variables?.[name]
    const given = variableValue(variable)
    if (typeof given === "string" && !/[{}]/.test(given)) {
      fixed[name] = given
      continue
    }
    const isRequired = required || !!variable?.isRequired
    fields.push({
      key: `${key}:${name}`,
      label: name,
      description: variable?.description ?? about,
      secret: variable?.isSecret ?? secret,
      required: isRequired,
      default: variable?.default,
    })
  }
  return {
    name: key,
    required,
    fields,
    fill(values) {
      let missing = false
      const resolved = value.replace(/\{([A-Za-z0-9._-]+)\}/g, (match, name: string) => {
        const field = fields.find((item) => item.key === `${key}:${name}`)
        const filled = fixed[name] ?? typed(field && (values[field.key] || field.default))
        if (filled === undefined) missing = true
        return filled ?? match
      })
      if (missing) return required ? null : undefined
      return safeValue(resolved) && !/[{}]/.test(resolved) ? resolved : null
    },
  }
}

function variableValue(variable: RegistryInput | undefined) {
  if (!variable) return variable

  return Object.prototype.hasOwnProperty.call(variable, "value") ? variable.value : undefined
}

function typed(value: string | undefined) {
  const trimmed = value?.trim()
  return trimmed ? trimmed : undefined
}

function fieldsOf(parts: Part[]) {
  const seen = new Set<string>()
  return parts.flatMap((part) => part.fields).filter((field) => !seen.has(field.key) && !!seen.add(field.key))
}

/** Named and positional arguments, each a part; `null` when the entry asks for something Drift will not build. */
function argumentParts(items: RegistryArgument[] | undefined, key: string) {
  const parts: (Part & { argument: RegistryArgument })[] = []
  for (const [index, item] of (items ?? []).entries()) {
    if (!validArgument(item)) return null
    if (item.type !== "named" && item.type !== "positional") return null
    // A named argument with nothing to put after it is a flag: present when the entry requires it, else left out.
    const flag = item.type === "named" && item.value === undefined && item.default === undefined && !item.variables
    if (flag) {
      if (item.isRequired) parts.push({ name: item.name!, required: true, fields: [], fill: () => "", argument: item })
      continue
    }
    const part = input(item, `${key}:${index}`, item.name ?? item.valueHint ?? `argument ${index + 1}`)
    if (!part) return null
    parts.push({ ...part, argument: item })
  }
  return parts
}

function validArgument(item: RegistryArgument) {
  return (
    !item.isRepeated &&
    !(item.type === "named" && (!item.name || !/^-{1,2}[A-Za-z0-9][A-Za-z0-9._-]*$/.test(item.name)))
  )
}

/** A double-dash flag takes its value after `=`, a single-dash one as the next word; an optional argument left empty is dropped. */
function filledArguments(parts: (Part & { argument: RegistryArgument })[], values: Record<string, string>) {
  const result: string[] = []
  for (const part of parts) {
    const value = part.fill(values)
    if (value === null) return null
    if (value === undefined) {
      if (part.required) return null
      continue
    }
    const { argument } = part
    if (argument.type === "positional") {
      if (value.startsWith("-")) return null
      result.push(value)
      continue
    }
    if (!value) result.push(argument.name!)
    else if (argument.name!.startsWith("--")) result.push(`${argument.name}=${value}`)
    else result.push(argument.name!, value)
  }
  return result
}

/** Headers or environment: one left empty is omitted when optional, so an environment variable comes from where the server starts. */
function filledPairs(
  parts: Part[] | undefined,
  values: Record<string, string>,
  shape = (_name: string, value: string) => value,
) {
  const result: Record<string, string> = {}
  for (const part of parts ?? []) {
    if (Object.prototype.hasOwnProperty.call(result, part.name)) return null
    const value = part.fill(values)
    if (value === null || (value === undefined && part.required)) return null
    if (value !== undefined) result[part.name] = shape(part.name, value)
  }
  return result
}

function safeValue(value: string) {
  return value.length <= 16_384 && !/[\0\r\n]/.test(value)
}

function validServerName(value: string) {
  return value.length <= 200 && /^[A-Za-z0-9.-]+\/[A-Za-z0-9._-]+$/.test(value)
}

function validHeaderName(value: string) {
  return value.length <= 256 && /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(value)
}

function validEnvironmentName(value: string) {
  return value.length <= 256 && /^[A-Za-z_][A-Za-z0-9_]*$/.test(value)
}

function validNpmIdentifier(value: string) {
  return value.length <= 214 && /^(?:@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/.test(value)
}

function validPypiIdentifier(value: string) {
  return value.length <= 200 && /^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$/.test(value)
}

function validImage(value: string) {
  return value.length <= 300 && /^[a-z0-9][a-z0-9._/-]*(?::[A-Za-z0-9._-]+)?(?:@sha256:[a-f0-9]{64})?$/.test(value)
}

function validHttpsUrl(value: string) {
  try {
    const url = new URL(value)
    return url.protocol === "https:" && !!url.hostname && !url.username && !url.password
  } catch {
    return false
  }
}

function isPinned(registry: string, version: string) {
  if (registry === "npm") {
    return /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(
      version,
    )
  }
  if (registry === "pypi") {
    return /^(?:[1-9]\d*!|0!?)?\d+(?:\.\d+)*(?:(?:[-_.]?(?:a|b|rc)\d*)?(?:-\d+|[-_.]?(?:post|rev|r)\d*)?(?:[-_.]?dev\d*)?)(?:\+[a-z0-9]+(?:[-_.][a-z0-9]+)*)?$/i.test(
      version,
    )
  }
  return registry === "oci"
}
