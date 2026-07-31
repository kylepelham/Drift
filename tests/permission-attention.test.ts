import { afterEach, expect, test } from "bun:test"
import type { Permission } from "@opencode-ai/sdk/client"
import { createEngineState } from "../src/engine/store"
import {
  beginPermissionReply,
  clearPermissionAttention,
  failPermissionReply,
  observePermission,
  permissionRequiresAttention,
  type DriftPermission,
} from "../src/state/permission-attention"

const ids = new Set<string>()
afterEach(() => {
  for (const id of ids) clearPermissionAttention(id)
  ids.clear()
})

function permission(id: string, sessionID: string, type = "bash"): Permission {
  ids.add(id)
  return {
    id,
    sessionID,
    type,
    messageID: "m1",
    callID: id,
    title: type,
    metadata: {},
    time: { created: 1 },
  }
}

test("permission attention follows global, thread, child, and linked-child auto-accept", () => {
  const [state, set] = createEngineState()
  set("sessions", "child", { id: "child", parentID: "parent" } as never)
  set("links", "linked-child", "linked")

  expect(permissionRequiresAttention(permission("global", "other"), state, { global: true, sessions: [] })).toBeFalse()
  expect(permissionRequiresAttention(permission("thread", "thread"), state, { global: false, sessions: ["thread"] })).toBeFalse()
  expect(permissionRequiresAttention(permission("child", "child"), state, { global: false, sessions: ["parent"] })).toBeFalse()
  expect(
    permissionRequiresAttention(permission("linked-child", "linked-child"), state, { global: false, sessions: ["linked"] }),
  ).toBeFalse()
  expect(permissionRequiresAttention(permission("manual", "other"), state, { global: false, sessions: [] })).toBeTrue()
})

test("failed automatic replies become manual attention", () => {
  const [state] = createEngineState()
  const request = permission("automatic", "s1")
  beginPermissionReply(request, "once", [request])
  expect(permissionRequiresAttention(request, state, { global: false, sessions: [] })).toBeFalse()
  failPermissionReply(request.id)
  expect(permissionRequiresAttention(request, state, { global: true, sessions: [] })).toBeTrue()
})

test("a stuck automatic reply is promoted to manual attention", async () => {
  const [state] = createEngineState()
  const request = permission("stuck", "s1")
  beginPermissionReply(request, "once", [request], 1)
  expect(permissionRequiresAttention(request, state, { global: true, sessions: [] })).toBeFalse()
  await Bun.sleep(5)
  expect(permissionRequiresAttention(request, state, { global: true, sessions: [] })).toBeTrue()
})

test("always replies stabilize queued and immediately following matching requests", () => {
  const [state] = createEngineState()
  const original = permission("original", "s1")
  const queued = permission("queued", "s1")
  original.pattern = "src/first.ts"
  original.metadata.always = ["src/**"]
  queued.pattern = "src/second.ts"
  const unrelated = permission("unrelated", "s1")
  unrelated.pattern = "tests/**"
  beginPermissionReply(original, "always", [original, queued, unrelated])
  expect(permissionRequiresAttention(queued, state, { global: false, sessions: [] })).toBeFalse()
  expect(permissionRequiresAttention(unrelated, state, { global: false, sessions: [] })).toBeTrue()

  const immediate = permission("immediate", "s1")
  immediate.pattern = "src/third.ts"
  observePermission(immediate)
  expect(permissionRequiresAttention(immediate, state, { global: false, sessions: [] })).toBeFalse()
  failPermissionReply(immediate.id)
  expect(permissionRequiresAttention(immediate, state, { global: false, sessions: [] })).toBeTrue()
  observePermission(immediate)
  expect(permissionRequiresAttention(immediate, state, { global: false, sessions: [] })).toBeTrue()
})

test("v2 always grants stabilize matching requests across sessions in one location", () => {
  const [state] = createEngineState()
  const original = permission("v2-original", "s1") as DriftPermission
  original.driftProtocol = "v2"
  original.pattern = "git status"
  original.metadata = { directory: "C:/work", always: ["git *"] }
  const queued = permission("v2-queued", "s2") as DriftPermission
  queued.driftProtocol = "v2"
  queued.pattern = "git diff"
  queued.metadata.directory = "c:\\work\\"
  const legacy = permission("v1-queued", "s2")
  legacy.pattern = "git log"
  legacy.metadata.directory = "C:/work"

  beginPermissionReply(original, "always", [original, queued, legacy])
  expect(permissionRequiresAttention(queued, state, { global: false, sessions: [] })).toBeFalse()
  expect(permissionRequiresAttention(legacy, state, { global: false, sessions: [] })).toBeTrue()
})
