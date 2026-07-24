import type { McpConfig } from "./state/store"

export type RegistryInput = {
  value?: string
  default?: string
  isRequired?: boolean
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
export type RegistryServer = {
  name: string
  title?: string
  description: string
  version: string
  packages?: RegistryPackage[]
  remotes?: RegistryRemote[]
}

export function registryConfig(server: RegistryServer): McpConfig | null {
  if (!validServerName(server.name)) return null
  for (const remote of server.remotes ?? []) {
    const url = resolveTemplate(remote.url, remote.variables)
    if (!url || !validHttpsUrl(url)) continue
    const headers = registryKeyValues(remote.headers, validHeaderName)
    if (!headers) continue
    return { type: "remote", url, ...(Object.keys(headers).length ? { headers } : {}) }
  }

  for (const item of server.packages ?? []) {
    if (item.transport.type !== "stdio" || !isPinned(item.registryType, item.version)) continue
    const executable = item.registryType === "npm" ? "npx" : item.registryType === "pypi" ? "uvx" : ""
    if (!executable || (item.runtimeHint && item.runtimeHint !== executable)) continue
    if (item.registryType === "npm" ? !validNpmIdentifier(item.identifier) : !validPypiIdentifier(item.identifier))
      continue
    if (item.runtimeArguments?.length && item.runtimeHint !== executable) continue
    const runtimeArguments = registryArguments(item.runtimeArguments)
    const packageArguments = registryArguments(item.packageArguments)
    const environment = registryKeyValues(item.environmentVariables, validEnvironmentName)
    if (!runtimeArguments || !packageArguments || !environment) continue
    const reference =
      item.registryType === "npm" ? `${item.identifier}@${item.version}` : `${item.identifier}==${item.version}`
    return {
      type: "local",
      command: [executable, ...runtimeArguments, reference, ...packageArguments],
      ...(Object.keys(environment).length ? { environment } : {}),
    }
  }
  return null
}

function registryArguments(items: RegistryArgument[] | undefined) {
  const result: string[] = []
  for (const item of items ?? []) {
    if (item.isRepeated) return null
    const value = resolveInput(item)
    if (value === undefined) {
      if (item.isRequired) return null
      continue
    }
    if (!safeValue(value)) return null
    if (item.type === "named") {
      if (!item.name || !/^-{1,2}[A-Za-z0-9][A-Za-z0-9._-]*$/.test(item.name)) return null
      result.push(value ? `${item.name}=${value}` : item.name)
      continue
    }
    if (item.type !== "positional" || value.startsWith("-")) return null
    result.push(value)
  }
  return result
}

function registryKeyValues<T extends RegistryInput & { name: string }>(
  items: T[] | undefined,
  validName: (name: string) => boolean,
) {
  const result: Record<string, string> = {}
  for (const item of items ?? []) {
    if (!validName(item.name) || item.name in result) return null
    const resolved = resolveInput(item)
    const value = resolved ?? (validEnvironmentName(item.name) ? `{env:${item.name}}` : undefined)
    if (value === undefined) {
      if (item.isRequired) return null
      continue
    }
    if (!safeValue(value)) return null
    result[item.name] = value
  }
  return result
}

function resolveInput(input: RegistryInput): string | undefined {
  const value = Object.prototype.hasOwnProperty.call(input, "value") ? input.value : input.default
  return typeof value === "string" ? resolveTemplate(value, input.variables) : undefined
}

function resolveTemplate(value: string, variables: Record<string, RegistryInput> | undefined): string | undefined {
  let valid = true
  const resolved: string = value.replace(/\{([A-Za-z0-9._-]+)\}/g, (match, name: string): string => {
    const replacement: string | undefined = variables?.[name] ? resolveInput(variables[name]) : undefined
    if (replacement === undefined) {
      valid = false
      return match
    }
    return replacement
  })
  return valid && !/[{}]/.test(resolved) && safeValue(resolved) ? resolved : undefined
}

function safeValue(value: string) {
  return value.length <= 16_384 && !/[\0\r\n]/.test(value)
}

function validServerName(value: string) {
  return value.length <= 128 && /^[A-Za-z0-9.-]+\/[A-Za-z0-9._-]+$/.test(value)
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
  return false
}
