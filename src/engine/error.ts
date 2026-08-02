export type EngineError = { name?: string; data?: unknown }

export type RecoverableErrorKind = "usage" | "rate_limit" | "unavailable" | "provider_auth" | "transient"

export type RecoverableError = {
  kind: RecoverableErrorKind
  reason: string
}

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

export function classifyRecoverableError(error?: EngineError): RecoverableError | null {
  if (!error) return null
  const name = (error.name ?? "").toLowerCase()
  const data = record(error.data) ? error.data : {}
  const reason = errorText(error).trim()
  const text = `${name} ${reason} ${stringValue(data.responseBody)}`.toLowerCase()
  const status = typeof data.statusCode === "number" ? data.statusCode : undefined

  if (/abort|cancel(?:led|ation)?|interrupt/.test(name) || /\b(?:user )?cancel(?:led|ed)?\b|\babort(?:ed)?\b/.test(text))
    return null
  if (/contextoverflow|contentfilter|structuredoutput|messageoutputlength|invalidrequest/.test(name)) return null
  if (/context (?:window|length)|content filter|structured output|invalid request|malformed/.test(text)) return null

  if (name === "providerautherror" || status === 401 || status === 403 || authFailure(text))
    return recoverable("provider_auth", reason)
  if (status === 429 || /rate[_ -]?limit|too many requests|requests per (?:minute|day)|\b429\b/.test(text))
    return recoverable("rate_limit", reason)
  if (/usage limit|quota|insufficient[_ -]?quota|credit balance|billing|spend limit|out of credits/.test(text))
    return recoverable("usage", reason)
  if (
    status === 404 ||
    status === 502 ||
    status === 503 ||
    status === 504 ||
    /model (?:is )?(?:unavailable|not found|disabled)|no available (?:model|provider)|provider unavailable|service unavailable|overload|capacity/.test(
      text,
    )
  )
    return recoverable("unavailable", reason)
  if (
    data.isRetryable === true ||
    status === 408 ||
    (status !== undefined && status >= 500) ||
    /temporar|timeout|timed out|network|connection (?:closed|reset)|econnreset|socket hang up|try again/.test(text)
  )
    return recoverable("transient", reason)
  return null
}

function authFailure(text: string) {
  return /authentication|unauthori[sz]ed|invalid api key|api key.*invalid|credentials? (?:are )?(?:invalid|missing|expired)/.test(text)
}

function recoverable(kind: RecoverableErrorKind, reason: string): RecoverableError {
  return { kind, reason: reason || "The model request was interrupted" }
}

function stringValue(value: unknown) {
  return typeof value === "string" ? value : ""
}

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value)
}
