import type { Event, Message, Part, Permission, Session } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import type { EngineState } from "./store"

type Set = SetStoreFunction<EngineState>

export function reduce(set: Set, event: Event) {
  switch (event.type) {
    case "session.created":
    case "session.updated":
      return upsertSession(set, event.properties.info)
    case "session.deleted":
      return dropSession(set, event.properties.info)
    case "session.status":
      return set("status", event.properties.sessionID, event.properties.status)
    case "session.idle":
      return set("status", event.properties.sessionID, { type: "idle" })
    case "session.error":
      return recordError(set, event.properties.sessionID, event.properties.error)
    case "message.updated":
      return upsertMessage(set, event.properties.info)
    case "message.removed":
      return dropMessage(set, event.properties.sessionID, event.properties.messageID)
    case "message.part.updated":
      return upsertPart(set, event.properties.part)
    case "message.part.removed":
      return dropPart(set, event.properties)
    case "permission.updated":
      return addPermission(set, event.properties)
    case "permission.replied":
      return dropPermission(set, event.properties.sessionID, event.properties.permissionID)
    case "todo.updated":
      return set("todos", event.properties.sessionID, event.properties.todos)
  }
}

function upsertSession(set: Set, info: Session) {
  set("sessions", info.id, info)
}

function dropSession(set: Set, info: Session) {
  set(
    produce((s) => {
      delete s.sessions[info.id]
      delete s.transcripts[info.id]
      delete s.loaded[info.id]
      delete s.permissions[info.id]
      delete s.todos[info.id]
      delete s.status[info.id]
    }),
  )
}

function recordError(set: Set, sessionID?: string, error?: { name: string; data?: unknown }) {
  if (!sessionID || !error) return
  if (error.name === "MessageAbortedError") return
  const data = error.data as { message?: string } | undefined
  set("errors", sessionID, data?.message ?? error.name)
}

function upsertMessage(set: Set, info: Message) {
  set(
    produce((s) => {
      const list = s.loaded[info.sessionID] ? s.transcripts[info.sessionID] : undefined
      if (!list) return
      const index = list.findIndex((entry) => entry.info.id === info.id)
      if (index >= 0) list[index].info = info
      else list.push({ info, parts: [] })
    }),
  )
}

function dropMessage(set: Set, sessionID: string, messageID: string) {
  set(
    produce((s) => {
      const list = s.transcripts[sessionID]
      if (list) s.transcripts[sessionID] = list.filter((entry) => entry.info.id !== messageID)
    }),
  )
}

function upsertPart(set: Set, part: Part) {
  set(
    produce((s) => {
      const entry = s.transcripts[part.sessionID]?.find((item) => item.info.id === part.messageID)
      if (!entry) return
      const index = entry.parts.findIndex((existing) => existing.id === part.id)
      if (index >= 0) entry.parts[index] = part
      else entry.parts.push(part)
    }),
  )
}

function dropPart(set: Set, ref: { sessionID: string; messageID: string; partID: string }) {
  set(
    produce((s) => {
      const entry = s.transcripts[ref.sessionID]?.find((item) => item.info.id === ref.messageID)
      if (entry) entry.parts = entry.parts.filter((part) => part.id !== ref.partID)
    }),
  )
}

function addPermission(set: Set, permission: Permission) {
  set(
    produce((s) => {
      const list = s.permissions[permission.sessionID] ?? []
      if (!list.some((existing) => existing.id === permission.id)) list.push(permission)
      s.permissions[permission.sessionID] = list
    }),
  )
}

function dropPermission(set: Set, sessionID: string, permissionID: string) {
  set(
    produce((s) => {
      const list = s.permissions[sessionID]
      if (list) s.permissions[sessionID] = list.filter((permission) => permission.id !== permissionID)
    }),
  )
}
