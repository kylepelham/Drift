import { afterEach, beforeEach, expect, test } from "bun:test"
import { activateModal } from "../src/ui/modal"

// This mock dispatches retargeted focus/key events, not browser shadow-DOM Tab
// navigation. Tests explicitly supply the browser's next focus destination.
class FakeElement {
  parentElement: FakeElement | null = null
  children: FakeElement[] = []
  attributes = new Map<string, string>()
  style = { cssText: "" }
  tabIndex = -1
  hidden = false
  inert = false
  disabled = false
  visible = true
  zIndex = "50"
  focusCalls = 0

  constructor(readonly tagName = "div") {
    if (tagName === "button") this.tabIndex = 0
  }

  get isConnected(): boolean {
    return this === doc.body || !!this.parentElement?.isConnected
  }

  append(child: FakeElement) {
    child.parentElement = this
    this.children.push(child)
  }

  prepend(child: FakeElement) {
    child.parentElement = this
    this.children.unshift(child)
  }

  remove() {
    if (this.parentElement) this.parentElement.children = this.parentElement.children.filter((child) => child !== this)
    this.parentElement = null
  }

  contains(target: FakeElement | null): boolean {
    return target === this || this.children.some((child) => child.contains(target))
  }

  setAttribute(name: string, value: string) {
    this.attributes.set(name, value)
  }

  hasAttribute(name: string) {
    return this.attributes.has(name)
  }

  matches(selector: string) {
    expect(selector).toBe("audio[controls], video[controls]")
    return ["audio", "video"].includes(this.tagName) && this.hasAttribute("controls")
  }

  closest(selector: string): FakeElement | null {
    if (selector === "[data-modal-layer]" && this.hasAttribute("data-modal-layer")) return this
    return this.parentElement?.closest(selector) ?? null
  }

  querySelectorAll(selector: string): FakeElement[] {
    return this.children.flatMap((child) => {
      const matches = selector === "[autofocus]"
        ? child.hasAttribute("autofocus")
        : (child.tagName === "button" && !child.disabled) || child.tabIndex >= 0
      return [...(matches ? [child] : []), ...child.querySelectorAll(selector)]
    })
  }

  querySelector(selector: string) {
    return this.querySelectorAll(selector)[0] ?? null
  }

  getClientRects() {
    return this.visible ? [{}] : []
  }

  focus() {
    this.focusCalls++
    if (!this.isConnected || this.disabled || this.hidden || !this.visible) return
    if (doc.activeElement === this) return
    doc.activeElement = this
    const event = new Event("focusin")
    Object.defineProperty(event, "target", { value: this })
    doc.dispatchEvent(event)
  }
}

class FakeDocument extends EventTarget {
  body = new FakeElement("body")
  activeElement: FakeElement | null = null
  listeners = new Map<string, Set<EventListenerOrEventListenerObject>>()

  createElement(tag: string) {
    return new FakeElement(tag)
  }

  override addEventListener(type: string, listener: EventListenerOrEventListenerObject | null, options?: AddEventListenerOptions | boolean) {
    super.addEventListener(type, listener, options)
    if (!listener) return
    if (!this.listeners.has(type)) this.listeners.set(type, new Set())
    this.listeners.get(type)!.add(listener)
  }

  override removeEventListener(type: string, listener: EventListenerOrEventListenerObject | null, options?: EventListenerOptions | boolean) {
    super.removeEventListener(type, listener, options)
    if (listener) this.listeners.get(type)?.delete(listener)
    if (!this.listeners.get(type)?.size) this.listeners.delete(type)
  }
}

let doc: FakeDocument
const globalNames = ["document", "HTMLElement", "getComputedStyle"] as const
let originalGlobals: (PropertyDescriptor | undefined)[]
let cleanups: (() => void)[]

beforeEach(() => {
  originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name))
  doc = new FakeDocument()
  cleanups = []
  const values = [doc, FakeElement, (element: FakeElement) => ({ zIndex: element.zIndex })]
  globalNames.forEach((name, index) => {
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value: values[index] })
  })
})

afterEach(async () => {
  try {
    for (const cleanup of cleanups.reverse()) cleanup()
    await Promise.resolve()
    expect(doc.listeners.size).toBe(0)
  } finally {
    globalNames.forEach((name, index) => {
      const descriptor = originalGlobals[index]
      if (descriptor) Object.defineProperty(globalThis, name, descriptor)
      else Reflect.deleteProperty(globalThis, name)
    })
  }
})

function dialog(...children: FakeElement[]) {
  const element = new FakeElement()
  element.setAttribute("data-modal-layer", "")
  for (const child of children) element.append(child)
  doc.body.append(element)
  return element
}

function activate(element: FakeElement, nativeTabOrder?: boolean, onClose = () => {}) {
  const cleanup = activateModal(element as unknown as HTMLElement, onClose, { nativeTabOrder })
  cleanups.push(cleanup)
  return cleanup
}

function key(key: string, shiftKey = false) {
  const event = Object.assign(new Event("keydown", { cancelable: true }), { key, shiftKey })
  Object.defineProperty(event, "target", { value: doc.activeElement })
  doc.dispatchEvent(event)
  return event
}

test("default traversal still prevents every Tab and wraps without guards or focusin listeners", async () => {
  const first = new FakeElement("button")
  const last = new FakeElement("button")
  const element = dialog(first, last)
  activate(element)
  await Promise.resolve()
  expect(doc.activeElement).toBe(first)
  expect(element.children).toEqual([first, last])
  expect(doc.listeners.has("focusin")).toBe(false)
  expect(key("Tab").defaultPrevented).toBe(true)
  expect(doc.activeElement).toBe(last)
  key("Tab")
  expect(doc.activeElement).toBe(first)
  key("Tab", true)
  expect(doc.activeElement).toBe(last)
})

test.each(["audio", "video"])("native %s traversal leaves Tab and Shift+Tab to the browser, wrapping only at guards", async (tag) => {
  const first = new FakeElement("button")
  const media = new FakeElement(tag)
  media.tabIndex = 0
  media.setAttribute("controls", "")
  const element = dialog(first, media)
  activate(element, true)
  await Promise.resolve()
  const [start, , , end] = element.children
  expect(start.hasAttribute("data-modal-focus-guard")).toBe(true)
  expect(end.hasAttribute("data-modal-focus-guard")).toBe(true)
  for (const guard of [start, end]) {
    expect(guard.tabIndex).toBe(0)
    expect(guard.hasAttribute("aria-hidden")).toBe(false)
  }
  expect(doc.activeElement).toBe(first)
  expect(key("Tab").defaultPrevented).toBe(false)
  media.focus()
  // Multiple internal controls retarget to the same media host. None may be skipped.
  for (const shift of [false, false, true, true]) {
    expect(key("Tab", shift).defaultPrevented).toBe(false)
    expect(doc.activeElement).toBe(media)
  }
  end.focus()
  expect(doc.activeElement).toBe(first)
  expect(key("Tab", true).defaultPrevented).toBe(false)
  start.focus()
  expect(doc.activeElement).toBe(end)
  expect(key("ArrowRight").defaultPrevented).toBe(false)
  expect(key(" ").defaultPrevented).toBe(false)
})

test("native guards handle empty dialogs and query newly loaded media at wrap time", async () => {
  const element = dialog()
  activate(element, true)
  await Promise.resolve()
  const [start, end] = element.children
  expect(doc.activeElement).toBe(element)
  start.focus()
  expect(doc.activeElement).toBe(element)
  end.focus()
  expect(doc.activeElement).toBe(element)
  const content = new FakeElement()
  element.children.splice(1, 0, content)
  content.parentElement = element
  const media = new FakeElement("audio")
  media.tabIndex = 0
  media.setAttribute("controls", "")
  content.append(media)
  start.focus()
  expect(doc.activeElement).toBe(end)
  media.remove()
  key("Tab")
  expect(doc.activeElement).toBe(element)
})

test.each(["audio", "video"])("reverse wrap parks after %s without focusing its host, then allows native reverse entry", async (tag) => {
  const first = new FakeElement("button")
  const close = new FakeElement("button")
  const media = new FakeElement(tag)
  media.tabIndex = 0
  media.setAttribute("controls", "")
  const element = dialog(first, close, media)
  activate(element, true)
  await Promise.resolve()
  const start = element.children[0]
  const end = element.children.at(-1)!

  expect(key("Tab", true).defaultPrevented).toBe(false)
  start.focus()
  // The end guard's synchronous focusin must not bounce back to the first button.
  expect(doc.activeElement).toBe(end)
  expect(end.hasAttribute("aria-hidden")).toBe(false)
  expect(media.focusCalls).toBe(0)
  expect(key("Tab", true).defaultPrevented).toBe(false)
  expect(doc.activeElement).toBe(end)
  expect(media.focusCalls).toBe(0)

  // Simulate the browser entering the last shadow control, retargeted to its host.
  media.focus()
  for (let index = 0; index < 3; index++) {
    expect(key("Tab", true).defaultPrevented).toBe(false)
    expect(doc.activeElement).toBe(media)
  }
  expect(media.focusCalls).toBe(1)
  close.focus()
  expect(doc.activeElement).toBe(close)
  // Once native focus moves off the guard, ordinary forward wrapping still works.
  end.focus()
  expect(doc.activeElement).toBe(first)
})

test("forward Tab from the parked end guard wraps immediately instead of escaping", async () => {
  const first = new FakeElement("button")
  const media = new FakeElement("audio")
  media.tabIndex = 0
  media.setAttribute("controls", "")
  const element = dialog(first, media)
  activate(element, true)
  await Promise.resolve()
  const [start, , , end] = element.children
  start.focus()
  expect(doc.activeElement).toBe(end)
  expect(key("Tab").defaultPrevented).toBe(true)
  expect(doc.activeElement).toBe(first)
  expect(media.focusCalls).toBe(0)
  expect(key("Tab").defaultPrevented).toBe(false)
})

test("reverse wrapping to an ordinary control does not add a parked focus stop", async () => {
  const first = new FakeElement("button")
  const last = new FakeElement("button")
  const element = dialog(first, last)
  activate(element, true)
  await Promise.resolve()
  element.children[0].focus()
  expect(doc.activeElement).toBe(last)
})

test("Escape and cleanup work while focus is parked after media", async () => {
  const opener = new FakeElement("button")
  doc.body.append(opener)
  opener.focus()
  const first = new FakeElement("button")
  const media = new FakeElement("video")
  media.tabIndex = 0
  media.setAttribute("controls", "")
  const element = dialog(first, media)
  let closes = 0
  const cleanup = activate(element, true, () => closes++)
  await Promise.resolve()
  element.children[0].focus()
  expect(doc.activeElement).toBe(element.children.at(-1)!)
  expect(key("Escape").defaultPrevented).toBe(true)
  expect(closes).toBe(1)
  cleanup()
  await Promise.resolve()
  expect(doc.activeElement).toBe(opener)
  expect(doc.listeners.size).toBe(0)
  expect(element.children).toEqual([first, media])
})

test("native focus capture recovers outside focus, including after the previous target is removed", async () => {
  const outside = new FakeElement("button")
  doc.body.append(outside)
  const first = new FakeElement("button")
  const last = new FakeElement("button")
  const element = dialog(first, last)
  activate(element, true)
  await Promise.resolve()
  last.focus()
  outside.focus()
  expect(doc.activeElement).toBe(last)
  last.remove()
  outside.focus()
  expect(doc.activeElement).toBe(first)
  first.disabled = true
  outside.focus()
  expect(doc.activeElement).toBe(element)
})

test("a guard falls back to the dialog when a tabindex destination cannot receive focus", async () => {
  const button = new FakeElement("button")
  const element = dialog(button)
  activate(element, true)
  await Promise.resolve()
  // Disabled elements with an explicit tabindex can still match the selector.
  button.disabled = true
  const [start, , end] = element.children
  end.focus()
  expect(doc.activeElement).toBe(element)
  start.focus()
  expect(doc.activeElement).toBe(element)
})

test("native mode honors autofocus and consumes Escape before downstream listeners", async () => {
  const first = new FakeElement("button")
  const media = new FakeElement("video")
  media.tabIndex = 0
  media.setAttribute("autofocus", "")
  const element = dialog(first, media)
  let closes = 0
  activate(element, true, () => closes++)
  await Promise.resolve()
  expect(doc.activeElement).toBe(media)
  let downstream = 0
  const listener = () => downstream++
  doc.addEventListener("keydown", listener, true)
  try {
    expect(key("Escape").defaultPrevented).toBe(true)
    expect(closes).toBe(1)
    expect(downstream).toBe(0)
  } finally {
    doc.removeEventListener("keydown", listener, true)
  }
})

test.each([false, true])("only the top modal handles focus and Escape, with native top = %s", async (nativeTop) => {
  const lowerButton = new FakeElement("button")
  const lower = dialog(lowerButton)
  let lowerCloses = 0
  activate(lower, true, () => lowerCloses++)
  await Promise.resolve()
  const upperButton = new FakeElement("button")
  const upper = dialog(upperButton)
  let upperCloses = 0
  const closeUpper = activate(upper, nativeTop, () => upperCloses++)
  await Promise.resolve()
  expect(doc.activeElement).toBe(upperButton)
  expect(lower.inert).toBe(true)
  expect(key("Escape").defaultPrevented).toBe(true)
  expect(upperCloses).toBe(1)
  expect(lowerCloses).toBe(0)
  closeUpper()
  await Promise.resolve()
  expect(doc.activeElement).toBe(lowerButton)
  expect(lower.inert).toBe(false)
  key("Escape")
  expect(lowerCloses).toBe(1)
})

test("native cleanup removes guards/listeners, restores inert state and opener, and is idempotent", async () => {
  const opener = new FakeElement("button")
  const alreadyInert = new FakeElement()
  alreadyInert.inert = true
  doc.body.append(opener)
  doc.body.append(alreadyInert)
  opener.focus()
  const button = new FakeElement("button")
  const element = dialog(button)
  const cleanup = activate(element, true)
  await Promise.resolve()
  expect(opener.inert).toBe(true)
  cleanup()
  cleanup()
  await Promise.resolve()
  expect(element.children).toEqual([button])
  expect(doc.listeners.size).toBe(0)
  expect(opener.inert).toBe(false)
  expect(alreadyInert.inert).toBe(true)
  expect(doc.activeElement).toBe(opener)
  expect(key("Tab").defaultPrevented).toBe(false)
})

test("cleanup before initial focus and out-of-order disposal cannot steal top-modal focus", async () => {
  const lower = dialog(new FakeElement("button"))
  const cleanupLower = activate(lower, true)
  const upperButton = new FakeElement("button")
  const upper = dialog(upperButton)
  activate(upper, true)
  cleanupLower()
  await Promise.resolve()
  expect(doc.activeElement).toBe(upperButton)
  expect(lower.children).toHaveLength(1)
  expect(doc.listeners.get("keydown")?.size).toBe(1)
  expect(doc.listeners.get("focusin")?.size).toBe(1)
})
