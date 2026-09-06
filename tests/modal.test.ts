import { expect, test } from "bun:test"
import { closeOnBackdropPointerDown, ModalStack } from "../src/ui/modal"

test("modal backdrop closes only when the pointer starts outside the dialog", () => {
  const backdrop = {}
  const dialog = {}
  let closes = 0
  let prevented = 0
  const close = () => closes++
  const preventDefault = () => prevented++

  closeOnBackdropPointerDown({ target: dialog, currentTarget: backdrop, preventDefault }, close)
  expect(closes).toBe(0)
  expect(prevented).toBe(0)

  closeOnBackdropPointerDown({ target: backdrop, currentTarget: backdrop, preventDefault }, close)
  expect(closes).toBe(1)
  expect(prevented).toBe(1)
})

test("modal stack keeps only the newest overlay topmost and tolerates out-of-order cleanup", () => {
  const stack = new ModalStack<object>()
  const settings = {}
  const editor = {}
  const lightbox = {}
  stack.push(settings)
  stack.push(editor)
  stack.push(lightbox)
  expect(stack.size).toBe(3)
  expect(stack.isTop(lightbox)).toBeTrue()
  expect(stack.isTop(settings)).toBeFalse()
  expect(stack.remove(editor)).toBeFalse()
  expect(stack.isTop(lightbox)).toBeTrue()
  expect(stack.remove(lightbox)).toBeTrue()
  expect(stack.isTop(settings)).toBeTrue()
  expect(stack.remove(settings)).toBeTrue()
  expect(stack.top()).toBeUndefined()
})

test("modal stack keeps a higher visual layer topmost over newer lower layers", () => {
  const stack = new ModalStack<{ priority: number }>((entry) => entry.priority)
  const lightbox = { priority: 50 }
  const settings = { priority: 30 }
  const editor = { priority: 40 }
  stack.push(lightbox)
  stack.push(settings)
  stack.push(editor)
  expect(stack.isTop(lightbox)).toBeTrue()
  expect(stack.remove(editor)).toBeFalse()
  expect(stack.remove(lightbox)).toBeTrue()
  expect(stack.isTop(settings)).toBeTrue()
})

test("settings, model manager, and split MCP dialogs use the shared modal lifecycle", async () => {
  const settings = await Bun.file("src/ui/settings.tsx").text()
  const models = await Bun.file("src/ui/model-manager.tsx").text()
  const mcp = await Bun.file("src/ui/mcp.tsx").text()
  const editor = await Bun.file("src/ui/mcp/editor.tsx").text()
  expect(settings).toContain("<Portal>")
  expect(settings).toContain("activateModal(dialog, props.onClose)")
  expect(settings).not.toContain('document.addEventListener("keydown", escape)')
  expect(models).toContain("<Portal>")
  expect(models).toContain("w-[min(35rem,calc(100vw-1rem))]")
  expect(mcp).toContain("activateModal(dialog, props.onClose)")
  expect(editor).toContain("activateModal(dialog, props.onClose)")
})
