import type { Permission } from "@opencode-ai/sdk/client"
import { createSignal } from "solid-js"
import type { EngineState } from "../engine/store"
import { autoAcceptAllowed, autoAcceptGlobal, autoAcceptSessions } from "./prefs"

type Marker = { kind: "replying" | "settling" | "failed"; token: number }
type Policy = { global: boolean; sessions: string[] }
export type DriftPermission = Permission & { driftProtocol?: "v2" }
type AlwaysRule = {
  protocol: "v1" | "v2"
  sessionID: string
  directory: string
  type: string
  patterns: string[]
  expires: number
}

const settleMs = 350
const lineageSettleMs = 500
const replyWatchdogMs = 8000
let sequence = 0
let alwaysRules: AlwaysRule[] = []
const [markers, setMarkers] = createSignal<Record<string, Marker>>({})

function policyAllowed(permission: Permission, state: EngineState, policy?: Policy) {
  return autoAcceptAllowed(
    policy?.global ?? autoAcceptGlobal(),
    policy?.sessions ?? autoAcceptSessions(),
    permission.sessionID,
    state.sessions[permission.sessionID]?.parentID,
    state.links[permission.sessionID],
  )
}

export function permissionRequiresAttention(permission: Permission, state: EngineState, policy?: Policy) {
  const marker = markers()[permission.id]
  if (marker?.kind === "failed") return true
  if (marker) return false
  return !policyAllowed(permission, state, policy)
}

export function permissionShouldAutoReply(permission: Permission, state: EngineState) {
  return !markers()[permission.id] && policyAllowed(permission, state)
}

function setMarker(id: string, kind: Marker["kind"], timeout?: number) {
  const marker = { kind, token: ++sequence } as const
  setMarkers((current) => ({ ...current, [id]: marker }))
  if (timeout)
    setTimeout(() => {
      setMarkers((current) => {
        if (current[id]?.token !== marker.token) return current
        return { ...current, [id]: { kind: "failed", token: ++sequence } }
      })
    }, timeout)
  return marker
}

function settle(permission: Permission, timeout = settleMs) {
  const marker = setMarker(permission.id, "settling")
  setTimeout(() => {
    setMarkers((current) => {
      if (current[permission.id]?.token !== marker.token) return current
      const next = { ...current }
      delete next[permission.id]
      return next
    })
  }, timeout)
}

function related(rule: AlwaysRule, permission: Permission) {
  const requested = requestPatterns(permission)
  const protocol = (permission as DriftPermission).driftProtocol === "v2" ? "v2" : "v1"
  const patternsMatch = requested.length
    ? requested.every((value) => rule.patterns.some((pattern) => wildcardMatch(pattern, value)))
    : rule.patterns.some((pattern) => wildcardMatch(pattern, ""))
  return (
    rule.protocol === protocol &&
    (rule.protocol === "v2" ? rule.directory === directoryKey(permission) : rule.sessionID === permission.sessionID) &&
    rule.type === permission.type &&
    patternsMatch
  )
}

function directoryKey(permission: Permission) {
  const directory = permission.metadata.directory
  return typeof directory === "string" ? directory.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase() : ""
}

function requestPatterns(permission: Permission) {
  return [permission.pattern].flat().filter((value): value is string => typeof value === "string")
}

function wildcardMatch(pattern: string, value: string) {
  let expression = pattern
    .replaceAll("\\", "/")
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replaceAll("*", ".*")
    .replaceAll("?", ".")
  if (expression.endsWith(" .*")) expression = expression.slice(0, -3) + "( .*)?"
  const windows = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent)
  return new RegExp(`^${expression}$`, windows ? "si" : "s").test(value.replaceAll("\\", "/"))
}

function grantedPatterns(permission: Permission) {
  const always = permission.metadata.always
  if (Array.isArray(always) && always.every((value) => typeof value === "string")) return always
}

export function beginPermissionReply(
  permission: Permission,
  response: "once" | "always" | "reject",
  pending: Permission[],
  watchdog = replyWatchdogMs,
) {
  setMarker(permission.id, "replying", watchdog)
  if (response !== "always") return
  const patterns = grantedPatterns(permission)
  if (!patterns?.length) return
  const rule = {
    protocol: (permission as DriftPermission).driftProtocol === "v2" ? ("v2" as const) : ("v1" as const),
    sessionID: permission.sessionID,
    directory: directoryKey(permission),
    type: permission.type,
    patterns,
    expires: Date.now() + settleMs,
  }
  alwaysRules = [...alwaysRules.filter((item) => item.expires > Date.now()), rule]
  for (const candidate of pending) if (candidate.id !== permission.id && related(rule, candidate)) settle(candidate)
}

export function observePermission(permission: Permission, state?: EngineState) {
  if (markers()[permission.id]) return
  const now = Date.now()
  alwaysRules = alwaysRules.filter((rule) => rule.expires > now)
  if (alwaysRules.some((rule) => related(rule, permission))) {
    settle(permission)
    return
  }
  const sessions = autoAcceptSessions()
  if (
    state &&
    !autoAcceptGlobal() &&
    sessions.length > 0 &&
    !sessions.includes(permission.sessionID) &&
    !state.sessions[permission.sessionID]?.parentID &&
    !state.links[permission.sessionID]
  )
    settle(permission, lineageSettleMs)
}

export function failPermissionReply(id: string) {
  setMarker(id, "failed")
}

export function clearPermissionAttention(id: string) {
  setMarkers((current) => {
    if (!current[id]) return current
    const next = { ...current }
    delete next[id]
    return next
  })
}

export function clearPermissionAttentionFor(permissions: Permission[]) {
  const ids = new Set(permissions.map((permission) => permission.id))
  setMarkers((current) => {
    const next = Object.fromEntries(Object.entries(current).filter(([id]) => !ids.has(id)))
    return Object.keys(next).length === Object.keys(current).length ? current : next
  })
}

export function prunePermissionAttention(present: Set<string>) {
  setMarkers((current) => {
    const next = Object.fromEntries(Object.entries(current).filter(([id]) => present.has(id)))
    return Object.keys(next).length === Object.keys(current).length ? current : next
  })
}
