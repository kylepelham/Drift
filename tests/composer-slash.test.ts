import { afterEach, expect, mock, test } from "bun:test"
import * as solid from "solid-js/dist/solid.js"
import * as ts from "typescript"
import { parseSlash, slashItem, slashItems, slashPresets } from "../src/ui/slash"
import { applyMirroredSession, selectedSession } from "../src/state/selection"
import type { Engine } from "../src/engine"
import type { createSlashMenu } from "../src/ui/composer-slash"

const source = await Bun.file(new URL("../src/ui/composer-slash.ts", import.meta.url)).text()
const parsed = ts.createSourceFile("composer-slash.ts", source, ts.ScriptTarget.Latest, true)
const executable = parsed.statements.filter((node) => !ts.isImportDeclaration(node))
  .map((node) => node.getText(parsed).replace(/^export /, "")).join("\n")
const compiled = ts.transpileModule(executable, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText
const cleanups: (() => void)[] = []
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup() })

function setup(initial: string) {
  const previous = selectedSession()
  applyMirroredSession("session")
  cleanups.push(() => applyMirroredSession(previous))
  const [draft, setDraft] = solid.createSignal(initial)
  const execute = mock(async (..._args: unknown[]) => {})
  const area = {
    focus: mock(() => {}), setSelectionRange: mock(() => {}),
    get selectionStart() { return draft().length }, get selectionEnd() { return draft().length },
  }
  const engine = {
    state: { commands: [{
      name: "impeccable", description: "Design tools", source: "command", usage: "[audit|polish] [target]",
      agent: "build", subtask: true, template: 'Call skill({ name: "impeccable" }) and handle $ARGUMENTS.',
      subcommands: [
        { name: "audit", description: "Check accessibility", usage: "[target]" },
        { name: "polish", description: "Final quality pass", usage: "[target]" },
        ...Array.from({ length: 12 }, (_, i) => ({ name: `extra-${i}`, description: `Extra ${i}` })),
      ],
    }, { name: "plain", description: "No argument metadata" }] },
    refreshRuntimeMetadata: mock(async () => {}),
    actions: { notice: mock(() => {}) },
  } as unknown as Engine
  const dependencies = { ...solid, parseSlash, slashItem, slashItems, slashPresets, runSlash: execute, isDesktopShell: () => true }
  const options = { engine, area: () => area, draft, setDraft, resize: mock(() => {}) }
  const run = new Function(...Object.keys(dependencies), "options", `${compiled}\nreturn createSlashMenu(options);`)
  const menu = solid.createRoot((dispose: () => void) => {
    cleanups.push(dispose)
    return run(...Object.values(dependencies), options) as ReturnType<typeof createSlashMenu>
  })
  const key = (name: string, modifiers: Partial<KeyboardEvent> = {}) => {
    const event = { key: name, preventDefault: mock(() => {}), ...modifiers } as unknown as KeyboardEvent
    return { consumed: menu.handleKey(event), event }
  }
  return { menu, key, draft, setDraft, execute, engine, area }
}

test.each(["/new", "/plain", "/impecc"])("Tab completes %s without execution or clearing the draft", async (draft) => {
  const view = setup(draft)
  expect(view.key("Tab").consumed).toBe(true)
  expect(view.draft()).toBe(draft === "/impecc" ? "/impeccable " : `${draft} `)
  expect(view.execute).not.toHaveBeenCalled()
  await Promise.resolve()
  expect(view.area.focus).toHaveBeenCalled()
})

test("Tab completes an executable built-in preset and Enter runs it explicitly", () => {
  const view = setup("/fork a")
  view.key("Tab")
  expect(view.draft()).toBe("/fork active")
  expect(view.execute).not.toHaveBeenCalled()
  view.key("Enter")
  expect(view.execute).toHaveBeenCalledTimes(1)
  expect(view.execute.mock.calls[0][2]).toBe("active")
})

test("skill subcommands expose all choices, help, prefix filtering, and completion before execution", () => {
  const view = setup("/impeccable ")
  expect(view.menu.argumentPresets()).toHaveLength(14)
  for (let i = 0; i < 13; i++) view.key("ArrowDown")
  expect(view.menu.activePresetIndex()).toBe(13)
  view.setDraft("/impeccable po")
  expect(view.menu.activePresetIndex()).toBe(0)
  expect(view.menu.argumentPresets()[0].description).toBe("Final quality pass")
  view.key("Tab")
  expect(view.draft()).toBe("/impeccable polish ")
  expect(view.menu.argumentHelp()).toEqual({ usage: "[target]", description: "Final quality pass" })
  expect(view.menu.argumentPresets()).toHaveLength(0)
  view.key("Tab")
  expect(view.execute).not.toHaveBeenCalled()
  view.setDraft("/impeccable polish src/ui/composer.tsx")
  view.key("Enter")
  expect(view.execute.mock.calls[0][2]).toBe("polish src/ui/composer.tsx")
})

test("Tab preserves custom arguments while completing a partial command name", () => {
  const view = setup("/impecc audit src/ui")
  view.key("Tab")
  expect(view.draft()).toBe("/impeccable audit src/ui")
  view.key("Tab")
  expect(view.execute).not.toHaveBeenCalled()
})

test("Escape dismisses the menu and composition/modifier keys do not execute commands", () => {
  const view = setup("/plain")
  for (const modifiers of [{ shiftKey: true }, { isComposing: true }, { ctrlKey: true }, { altKey: true }])
    expect(view.key("Enter", modifiers).consumed).toBe(false)
  expect(view.execute).not.toHaveBeenCalled()
  view.key("Escape")
  expect(view.menu.open()).toBe(false)
  view.setDraft("/does-not-exist")
  view.menu.setDismissed(false)
  expect(view.key("Tab").consumed).toBe(false)
  expect(view.key("Enter").consumed).toBe(false)
})

test("command matching and exact lookup are not limited to the first eight entries", () => {
  const view = setup("/")
  expect(view.menu.matches().length).toBeGreaterThan(8)
  expect(slashItem(view.engine, "impeccable")?.name).toBe("impeccable")
})

test("argument details start collapsed and expand independently from command execution", () => {
  const view = setup("/impeccable ")
  expect(view.menu.expandedArgument()).toBeUndefined()
  view.key("ArrowRight")
  expect(view.menu.expandedArgument()).toBe("audit ")
  expect(view.draft()).toBe("/impeccable ")
  expect(view.execute).not.toHaveBeenCalled()
  view.key("ArrowLeft")
  expect(view.menu.expandedArgument()).toBeUndefined()
  view.menu.toggleArgumentHelp(view.menu.argumentPresets()[1])
  expect(view.menu.expandedArgument()).toBe("polish ")
  expect(view.execute).not.toHaveBeenCalled()
  view.setDraft("/impeccable pol")
  expect(view.menu.expandedArgument()).toBeUndefined()
})
