import { afterEach, beforeEach, expect, test } from "bun:test"
import { dragReorder } from "../src/ui/drag-reorder"

class TrackedTarget extends EventTarget {
  listeners = new Map<string, Set<EventListenerOrEventListenerObject>>()

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

class FakeElement extends TrackedTarget {
  style = { transform: "", transition: "", position: "", zIndex: "", top: "" }
  parentElement: FakeElement | null = null
  children: FakeElement[] = []
  scrollTop = 0
  offsetHeight = 40
  captures = new Set<number>()
  captureCount = 0
  releaseCount = 0
  rect = () => ({ top: 0, bottom: 40, height: 40 })

  constructor(readonly id: string) {
    super()
  }

  getBoundingClientRect() {
    return this.rect()
  }

  querySelectorAll(selector: string) {
    expect(selector).toBe(".group")
    return this.children
  }

  setPointerCapture(id: number) {
    this.captures.add(id)
    this.captureCount++
  }

  hasPointerCapture(id: number) {
    return this.captures.has(id)
  }

  releasePointerCapture(id: number) {
    this.captures.delete(id)
    this.releaseCount++
    this.dispatchEvent(pointer("lostpointercapture", 0, { pointerId: id }))
  }
}

function pointer(type: string, clientY: number, extra: Partial<PointerEvent> = {}) {
  return Object.assign(new Event(type), {
    pointerId: 7,
    clientY,
    button: 0,
    buttons: type === "pointerup" ? 0 : 1,
    isPrimary: true,
    pointerType: "mouse",
    ...extra,
  }) as PointerEvent
}

function translation(element: FakeElement) {
  return element.style.transform ? Number(element.style.transform.slice(11, -3)) : 0
}

function expectTranslation(element: FakeElement, value: number) {
  expect(element.style.transform).toMatch(/^translateY\(-?[\d.]+px\)$/)
  expect(translation(element)).toBeCloseTo(value, 8)
}

const globalNames = ["window", "getComputedStyle"] as const
let originalGlobals: (PropertyDescriptor | undefined)[]
let cancel = () => {}

beforeEach(() => {
  originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name))
  cancel = () => {}
})

afterEach(() => {
  try {
    cancel()
  } finally {
    globalNames.forEach((name, index) => {
      const descriptor = originalGlobals[index]
      if (descriptor) Object.defineProperty(globalThis, name, descriptor)
      else Reflect.deleteProperty(globalThis, name)
    })
  }
  globalNames.forEach((name, index) => {
    expect(Object.getOwnPropertyDescriptor(globalThis, name)).toEqual(originalGlobals[index])
  })
})

function setup({ scale = 1, scrollTop = 32, sticky = false, position = "" } = {}) {
  const win = new TrackedTarget()
  const container = new FakeElement("container")
  container.scrollTop = scrollTop
  const items = ["a", "b", "c", "d", "e"].map((id, index) => {
    const item = new FakeElement(id)
    item.parentElement = container
    item.rect = () => {
      // DOM rects are viewport pixels; scroll and inline translations are layout pixels.
      const top = 100 + (index * 48 - container.scrollTop + translation(item)) * scale
      return { top, bottom: top + 40 * scale, height: 40 * scale }
    }
    return item
  })
  container.children = items
  const root = items[1]
  const header = new FakeElement("header")
  header.parentElement = root
  header.style.position = position
  header.style.top = sticky ? "7px" : ""
  header.rect = () => {
    const naturalTop = root.getBoundingClientRect().top
    const offset = Number.parseFloat(header.style.top) || 0
    const top = header.style.position === "relative"
      ? naturalTop + offset * scale
      : sticky ? Math.max(naturalTop, 100 + offset * scale) : naturalTop
    return { top, bottom: top + 16 * scale, height: 16 * scale }
  }
  Object.defineProperty(globalThis, "window", { configurable: true, writable: true, value: win })
  Object.defineProperty(globalThis, "getComputedStyle", {
    configurable: true,
    writable: true,
    value: (element: FakeElement) => ({ position: element.style.position || (sticky && element === header ? "sticky" : "static") }),
  })
  const moves: [string, string | null][] = []
  let dragged = 0
  const startY = header.getBoundingClientRect().top + 4 * scale
  const down = (event: Event) => {
    cancel = dragReorder(event as PointerEvent, root as unknown as HTMLElement, {
      selector: ".group",
      id: "b",
      itemID: (element) => (element as unknown as FakeElement).id,
      move: (id, beforeID) => moves.push([id, beforeID]),
      dragged: () => dragged++,
    })
  }
  header.addEventListener("pointerdown", down)
  header.dispatchEvent(pointer("pointerdown", startY))
  header.removeEventListener("pointerdown", down)

  return {
    win, container, items, root, header, moves,
    move: (dy = 8, extra: Partial<PointerEvent> = {}) => win.dispatchEvent(pointer("pointermove", startY + dy * scale, extra)),
    up: (dy = 8) => win.dispatchEvent(pointer("pointerup", startY + dy * scale)),
    scroll: (top: number) => {
      container.scrollTop = top
      container.dispatchEvent(new Event("scroll"))
    },
    expectClean(committed = false) {
      expect(win.listeners.size).toBe(0)
      expect(container.listeners.size).toBe(0)
      expect(header.listeners.size).toBe(0)
      expect(header.captures.size).toBe(0)
      expect(header.releaseCount).toBe(header.captureCount)
      for (const item of items) {
        expect(item.style.transform).toBe("")
        expect(item.style.transition).toBe("")
      }
      expect(root.style.position).toBe("")
      expect(root.style.zIndex).toBe("")
      expect(header.style.position).toBe(position)
      expect(header.style.top).toBe(sticky ? "7px" : "")
      expect(moves).toHaveLength(committed ? 1 : 0)
      expect(dragged).toBe(committed ? 1 : 0)
    },
  }
}

test.each([1, 0.8, 1.3])("stationary pointer follows scroll and updates the drop target at zoom %s", (scale) => {
  const drag = setup({ scale })
  drag.move()
  expectTranslation(drag.root, 8)
  expect(drag.header.captureCount).toBe(1)
  const top = drag.root.getBoundingClientRect().top

  drag.scroll(72)
  expectTranslation(drag.root, 48)
  expect(drag.root.getBoundingClientRect().top).toBeCloseTo(top, 8)
  expectTranslation(drag.items[2], -48)
  expect(drag.items[0].style.transform).toBe("")
  expect(drag.items[3].style.transform).toBe("")
  expect(drag.moves).toEqual([])
  drag.up()
  expect(drag.moves).toEqual([["b", "d"]])
  drag.expectClean(true)
})

test.each([1, 0.8, 1.3])("movement after scrolling adds pointer and scroll displacement at zoom %s", (scale) => {
  const drag = setup({ scale })
  drag.move()
  drag.scroll(72)
  const top = drag.root.getBoundingClientRect().top
  drag.move(48)
  expectTranslation(drag.root, 88)
  expect(drag.root.getBoundingClientRect().top - top).toBeCloseTo(40 * scale, 8)
  expectTranslation(drag.items[2], -48)
  expectTranslation(drag.items[3], -48)
  drag.up(48)
  expect(drag.moves).toEqual([["b", "e"]])
  drag.expectClean(true)
})

test.each([1, 0.8, 1.3])("pointerup reads changed scrollTop before a scroll event at zoom %s", (scale) => {
  const drag = setup({ scale })
  drag.move()
  drag.container.scrollTop = 112
  expectTranslation(drag.root, 8)
  drag.up()
  expect(drag.moves).toEqual([["b", "e"]])
  drag.expectClean(true)
})

test("pointerup uses its final pointer position as well as pending scroll", () => {
  const drag = setup()
  drag.move()
  drag.container.scrollTop = 72
  drag.up(48)
  expect(drag.moves).toEqual([["b", "e"]])
  drag.expectClean(true)
})

test.each([1, 0.8, 1.3])("reverse scroll clears old shifts and moves before the first item at zoom %s", (scale) => {
  const drag = setup({ scale, scrollTop: 120 })
  drag.move()
  drag.scroll(200)
  expectTranslation(drag.root, 88)
  expectTranslation(drag.items[3], -48)
  drag.scroll(120)
  expectTranslation(drag.root, 8)
  expect(drag.items[2].style.transform).toBe("")
  expect(drag.items[3].style.transform).toBe("")
  drag.scroll(72)
  expectTranslation(drag.root, -40)
  expectTranslation(drag.items[0], 48)
  drag.up()
  expect(drag.moves).toEqual([["b", "a"]])
  drag.expectClean(true)
})

for (const scale of [1, 0.8, 1.3]) {
  test.each([
    { scroll: 0, dy: -48, before: "a" },
    { scroll: 4000, dy: 144, before: null },
  ])("scroll clamps to list bounds at zoom " + scale + ": %j", ({ scroll, dy, before }) => {
    const drag = setup({ scale, scrollTop: 2000 })
    drag.move()
    drag.scroll(scroll)
    expectTranslation(drag.root, dy)
    drag.up()
    expect(drag.moves).toEqual([["b", before]])
    drag.expectClean(true)
  })
}

test("scroll before activation neither captures nor commits a drag", () => {
  const drag = setup()
  drag.scroll(200)
  drag.move(4)
  expect(drag.header.captureCount).toBe(0)
  expect(drag.root.style.transform).toBe("")
  expect(drag.root.style.position).toBe("")
  expect(drag.items[2].style.transition).toBe("")
  drag.up(4)
  drag.expectClean()
})

test("activation takes its scroll baseline after preactivation scrolling", () => {
  const drag = setup()
  drag.scroll(200)
  drag.move()
  expectTranslation(drag.root, 8)
  drag.scroll(240)
  expectTranslation(drag.root, 48)
  drag.up()
  expect(drag.moves).toEqual([["b", "d"]])
  drag.expectClean(true)
})

test.each(["pointercancel", "lostpointercapture", "blur", "manual", "releasedbuttons"])(
  "%s cancels an active scrolled drag and removes listeners, capture, and styles",
  (reason) => {
    const drag = setup({ sticky: true, scrollTop: 80 })
    drag.move()
    drag.scroll(120)
    expect(drag.root.style.position).toBe("relative")
    expect(drag.root.style.zIndex).toBe("10")
    expect(drag.items[2].style.transition).toBe("transform 150ms ease")
    expect(drag.header.style.position).toBe("relative")
    expectTranslation(drag.root, 48)
    if (reason === "manual") cancel()
    else if (reason === "releasedbuttons") drag.move(8, { buttons: 0 })
    else if (reason === "lostpointercapture") drag.header.releasePointerCapture(7)
    else drag.win.dispatchEvent(pointer(reason, 0))
    drag.expectClean()

    // Late events and repeated disposal must not reactivate or commit the old drag.
    drag.scroll(160)
    drag.move(80)
    drag.up(80)
    drag.header.dispatchEvent(pointer("lostpointercapture", 0))
    cancel()
    drag.expectClean()
  },
)

test("unrelated pointers cannot move, cancel, or commit the drag", () => {
  const drag = setup()
  drag.move()
  drag.scroll(72)
  drag.move(100, { pointerId: 99, buttons: 0 })
  drag.win.dispatchEvent(pointer("pointercancel", 0, { pointerId: 99 }))
  drag.win.dispatchEvent(pointer("pointerup", 0, { pointerId: 99 }))
  drag.header.dispatchEvent(pointer("lostpointercapture", 0, { pointerId: 99 }))
  expectTranslation(drag.root, 48)
  expect(drag.header.hasPointerCapture(7)).toBe(true)
  expect(drag.moves).toEqual([])
  drag.up()
  expect(drag.moves).toEqual([["b", "d"]])
  drag.expectClean(true)
})

for (const position of ["", "sticky"]) {
  test.each([1, 0.8, 1.3])("already-stuck header preserves its offset and restores inline position '" + position + "' at zoom %s", (scale) => {
    const drag = setup({ scale, sticky: true, scrollTop: 80, position })
    const top = drag.header.getBoundingClientRect().top
    expect(top).toBeGreaterThan(drag.root.getBoundingClientRect().top)
    drag.move()
    expect(drag.header.style.position).toBe("relative")
    expect(Number.parseFloat(drag.header.style.top)).toBeCloseTo(39, 8)
    expect(drag.header.getBoundingClientRect().top).toBeCloseTo(top + 8 * scale, 8)
    drag.scroll(120)
    expect(Number.parseFloat(drag.header.style.top)).toBeCloseTo(39, 8)
    expect(drag.header.getBoundingClientRect().top).toBeCloseTo(top + 8 * scale, 8)
    drag.up()
    expect(drag.moves).toEqual([["b", "d"]])
    drag.expectClean(true)
    expect(drag.header.getBoundingClientRect().top).toBeCloseTo(top, 8)
  })
}
