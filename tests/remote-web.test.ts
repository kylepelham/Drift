import { expect, test } from "bun:test"
import { backendRoute } from "../src/backend"
import { isNarrowWidth, navigationHash, parseNavigationHash } from "../src/state/navigation"
import { remoteStatusTone, type RemoteAccessStatus } from "../src/state/remote-access"
import { remoteEngineBase, remoteRuntimeFrom } from "../src/runtime"

test("remote runtime uses the same-origin engine gateway", () => {
  const remote = { pathname: "/companion", origin: "http://192.168.1.8:41718" }
  const desktop = { pathname: "/", origin: "http://localhost:5180" }
  expect(remoteRuntimeFrom(remote)).toBe(true)
  expect(remoteRuntimeFrom(desktop)).toBe(false)
  expect(remoteEngineBase(remote)).toBe("http://192.168.1.8:41718/engine")
  expect(remoteEngineBase(desktop)).toBeUndefined()
})

test("host backend routing prefers Tauri and otherwise uses remote RPC", () => {
  expect(backendRoute(true, true)).toBe("tauri")
  expect(backendRoute(false, true)).toBe("rpc")
  expect(backendRoute(false, false)).toBe("browser")
})

test("responsive navigation state round-trips and uses the narrow breakpoint", () => {
  expect(isNarrowWidth(390)).toBe(true)
  expect(isNarrowWidth(720)).toBe(false)
  const hash = navigationHash({ workspace: "project A", session: "ses_1", overlay: "drawer" })
  expect(parseNavigationHash(hash)).toEqual({ workspace: "project A", session: "ses_1", overlay: "drawer" })
})

test("remote settings state distinguishes online, offline, and error", () => {
  const base: RemoteAccessStatus = {
    enabled: true,
    listening: true,
    port: 41718,
    discoveryPort: 41717,
    urls: [],
    connectionUrls: [],
  }
  expect(remoteStatusTone(base)).toBe("online")
  expect(remoteStatusTone({ ...base, listening: false })).toBe("offline")
  expect(remoteStatusTone({ ...base, error: "busy" })).toBe("error")
})
