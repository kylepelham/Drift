import { expect, test } from "bun:test"
import { initDevtoolsShortcut, isDevtoolsShortcut } from "../src/state/devtools"

const chord = (overrides: Partial<Parameters<typeof isDevtoolsShortcut>[0]> = {}) => ({
  ctrlKey: true,
  shiftKey: true,
  altKey: false,
  metaKey: false,
  key: "I",
  ...overrides,
})

test("the DevTools chord requires exactly Ctrl+Shift+I", () => {
  expect(isDevtoolsShortcut(chord())).toBeTrue()
  expect(isDevtoolsShortcut(chord({ key: "i" }))).toBeTrue()
  expect(isDevtoolsShortcut(chord({ ctrlKey: false }))).toBeFalse()
  expect(isDevtoolsShortcut(chord({ shiftKey: false }))).toBeFalse()
  // AltGr layouts report Ctrl+Alt; those must never open the inspector while typing.
  expect(isDevtoolsShortcut(chord({ altKey: true }))).toBeFalse()
  expect(isDevtoolsShortcut(chord({ metaKey: true }))).toBeFalse()
  expect(isDevtoolsShortcut(chord({ key: "j" }))).toBeFalse()
})

test("the shortcut is desktop-only and inert without the Tauri bridge", () => {
  // No __TAURI__ in the test environment, so init must return a no-op cleanup and never touch
  // window listeners: browser and remote runtimes have their own DevTools.
  const cleanup = initDevtoolsShortcut()
  expect(cleanup).toBeFunction()
  expect(cleanup()).toBeUndefined()
})

test("release builds keep the devtools feature and command wired", async () => {
  const cargo = await Bun.file("src-tauri/Cargo.toml").text()
  expect(cargo).toMatch(/tauri = \{ version = "2", features = \["devtools"\] \}/)
  const main = await Bun.file("src-tauri/src/main.rs").text()
  expect(main).toContain("fn open_webview_devtools(window: tauri::WebviewWindow)")
  expect(main).toContain("open_webview_devtools,")
  const app = await Bun.file("src/app.tsx").text()
  expect(app).toContain("onCleanup(initDevtoolsShortcut())")
})
