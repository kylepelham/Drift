import { afterEach, expect, test } from "bun:test"
import { inspectShellEngine, inspectStatus, restartShellEngine } from "../src/engine/connection"

const previousTauri = (globalThis as { __TAURI__?: unknown }).__TAURI__
afterEach(() => {
  ;(globalThis as { __TAURI__?: unknown }).__TAURI__ = previousTauri
})

test("embedded engine status distinguishes ready, failed, and starting states", () => {
  expect(inspectStatus({ url: "http://127.0.0.1:4321", password: "secret" })).toEqual({
    target: {
      url: "http://127.0.0.1:4321",
      headers: { Authorization: `Basic ${btoa("opencode:secret")}` },
    },
  })
  expect(inspectStatus({ error: "embedded engine exited with code 1" })).toEqual({
    error: "embedded engine exited with code 1",
  })
  expect(inspectStatus({})).toEqual({})
})

test("shell engine inspection preserves the current terminal diagnostic", async () => {
  ;(globalThis as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: async () => ({ error: "embedded engine exited with code 1" }) },
  }

  expect(await inspectShellEngine()).toEqual({ error: "embedded engine exited with code 1" })
})

test("explicit engine restart waits for and returns the replacement target", async () => {
  const commands: string[] = []
  ;(globalThis as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: async (command: string) => {
        commands.push(command)
        if (command === "restart_engine") return null
        return { url: "http://127.0.0.1:5432", password: "replacement" }
      },
    },
  }

  expect(await restartShellEngine()).toEqual({
    url: "http://127.0.0.1:5432",
    headers: { Authorization: `Basic ${btoa("opencode:replacement")}` },
  })
  expect(commands).toEqual(["restart_engine", "engine_status"])
})
