export type EngineError = { name?: string; data?: unknown }

export function errorText(error?: EngineError) {
  if (!error) return "An error occurred"
  const data = error.data as { message?: unknown } | undefined
  const message = typeof data?.message === "string" ? data.message : ""
  return unwrapErrorMessage(message) || error.name || "An error occurred"
}

export function unwrapErrorMessage(message: string) {
  const text = message.replace(/^Error:\s*/, "").trim()
  const parse = (value: string) => {
    try {
      return JSON.parse(value) as unknown
    } catch {
      return undefined
    }
  }
  const read = (value: string) => {
    const first = parse(value)
    return typeof first === "string" ? parse(first.trim()) : first
  }
  let json = read(text)
  if (json === undefined) {
    const start = text.indexOf("{")
    const end = text.lastIndexOf("}")
    if (start !== -1 && end > start) json = read(text.slice(start, end + 1))
  }
  if (!record(json)) return message
  const nested = record(json.error) ? json.error : undefined
  if (nested) {
    const type = typeof nested.type === "string" ? nested.type : undefined
    const detail = typeof nested.message === "string" ? nested.message : undefined
    if (type && detail) return `${type}: ${detail}`
    if (detail) return detail
    if (type) return type
    if (typeof nested.code === "string") return nested.code
  }
  if (typeof json.message === "string") return json.message
  if (typeof json.error === "string") return json.error
  return message
}

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value)
}
