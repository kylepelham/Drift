import { afterEach, beforeEach, expect, mock, test } from "bun:test"

const globalNames = ["document", "NodeFilter", "Range", "Highlight", "CSS", "localStorage"] as const
let originalGlobals: (PropertyDescriptor | undefined)[]
let paintFindHighlights: typeof import("../src/ui/transcript-find").paintFindHighlights
let scrollFindOccurrence: typeof import("../src/ui/transcript-find").scrollFindOccurrence

class ElementDouble {
  open = false
  scrollIntoView = mock((_options: ScrollIntoViewOptions) => undefined)
  constructor(public tagName: string, public parentElement: ElementDouble | null = null, public ignored = false) {}
  closest(selector: string): ElementDouble | null {
    for (let node: ElementDouble | null = this; node; node = node.parentElement) {
      if (selector === "[data-find-ignore]" && node.ignored) return node
      if (selector === "details:not([open])" && node.tagName === "details" && !node.open) return node
    }
    return null
  }
}

type TextDouble = { nodeValue: string; parentElement: ElementDouble }
class RangeDouble {
  startContainer!: TextDouble
  startOffset!: number
  endContainer!: TextDouble
  endOffset!: number
  setStart(node: TextDouble, offset: number) { this.startContainer = node; this.startOffset = offset }
  setEnd(node: TextDouble, offset: number) { this.endContainer = node; this.endOffset = offset }
}

class HighlightDouble {
  constructor(...ranges: RangeDouble[]) { this.ranges = ranges }
  ranges: RangeDouble[]
}

const registry = new Map<string, HighlightDouble>()
const text = (nodeValue: string, parentElement: ElementDouble): TextDouble => ({ nodeValue, parentElement })
const row = (id: string, nodes: TextDouble[]) => ({ dataset: { mid: id }, nodes })
function container(...rows: ReturnType<typeof row>[]) {
  return { querySelectorAll: (selector: string) => {
    expect(selector).toBe("[data-mid]")
    return rows
  } } as unknown as HTMLElement
}

beforeEach(async () => {
  originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name))
  const set = (name: typeof globalNames[number], value: unknown) =>
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value })
  if (!("localStorage" in globalThis)) set("localStorage", { getItem: () => null, setItem: () => undefined })
  ;({ paintFindHighlights, scrollFindOccurrence } = await import("../src/ui/transcript-find"))
  registry.clear()
  set("CSS", { highlights: registry })
  set("Highlight", HighlightDouble)
  set("Range", RangeDouble)
  set("NodeFilter", { SHOW_TEXT: 4 })
  set("document", { createTreeWalker: (root: ReturnType<typeof row>, whatToShow: number) => {
    expect(whatToShow).toBe(4)
    let index = 0
    return { nextNode: () => root.nodes[index++] ?? null }
  } })
})

afterEach(() => {
  globalNames.forEach((name, index) => {
    const descriptor = originalGlobals[index]
    if (descriptor) Object.defineProperty(globalThis, name, descriptor)
    else Reflect.deleteProperty(globalThis, name)
  })
})

test("closed clarification highlights count only real QAs, not duplicate preview or empty-answer UI", () => {
  const details = new ElementDouble("details")
  const summary = new ElementDouble("summary", details, true)
  const preview = new ElementDouble("span", summary)
  const question = text("Blue or not Blue?", new ElementDouble("div", details))
  const answer = text("Blue, Blue", new ElementDouble("div", details))
  const empty = text("Blue", new ElementDouble("div", details, true))
  const root = container(row("u1", [text("Blue", summary), text("Blue, Blue", preview), question, answer, empty]))
  for (let index = 0; index < 4; index++) {
    const active = paintFindHighlights(root, "BLUE", { messageId: "u1", index })!
    expect(registry.get("drift-find")!.ranges).toHaveLength(4)
    expect(active.startContainer).toBe(index < 2 ? question : answer)
    expect(active.startOffset).toBe([0, 12, 0, 6][index])
    expect(active.endOffset - active.startOffset).toBe(4)
    expect(registry.get("drift-find-active")!.ranges).toEqual([active])
    expect(details.open).toBe(false)
  }
})

test("legacy and normal rows keep all their text and reset occurrence numbering per message", () => {
  const details = new ElementDouble("details")
  const legacy = text("Which color?\nBlue Blue\nUnanswered", new ElementDouble("div", details))
  const ordinary = text("Blue Blue", new ElementDouble("div"))
  const root = container(row("legacy", [legacy]), row("normal", [ordinary]))
  const active = paintFindHighlights(root, "blue", { messageId: "normal", index: 1 })!
  expect(registry.get("drift-find")!.ranges).toHaveLength(4)
  expect(active.startContainer).toBe(ordinary)
  expect(active.startOffset).toBe(5)
  expect(paintFindHighlights(root, "Unanswered", { messageId: "legacy", index: 0 })!.startContainer).toBe(legacy)
  expect(registry.get("drift-find")!.ranges).toHaveLength(1)
  expect(details.open).toBe(false)
})

test("navigation opens all closed ancestor details before scrolling; repaint respects a subsequent collapse", () => {
  const outer = new ElementDouble("details")
  const inner = new ElementDouble("details", outer)
  const parent = new ElementDouble("div", inner)
  parent.scrollIntoView.mockImplementation(() => {
    expect(inner.open).toBe(true)
    expect(outer.open).toBe(true)
  })
  const root = container(row("u1", [text("Blue Blue", parent)]))
  const occurrence = { messageId: "u1", index: 0 }
  const active = paintFindHighlights(root, "blue", occurrence)!
  scrollFindOccurrence(active)
  expect(parent.scrollIntoView.mock.calls).toEqual([[{ block: "nearest" }]])
  inner.open = false
  paintFindHighlights(root, "blue", occurrence)
  expect(inner.open).toBe(false)
  expect(parent.scrollIntoView).toHaveBeenCalledTimes(1)
  scrollFindOccurrence(paintFindHighlights(root, "blue", { ...occurrence, index: 1 })!)
  expect(inner.open).toBe(true)
  expect(parent.scrollIntoView).toHaveBeenCalledTimes(2)
})

test("ordinary navigation still scrolls, missing active rows do not highlight, and clearing removes ranges", () => {
  const parent = new ElementDouble("div")
  const root = container(row("u1", [text("Blue", parent)]))
  scrollFindOccurrence(paintFindHighlights(root, "blue", { messageId: "u1", index: 0 })!)
  expect(parent.scrollIntoView.mock.calls).toEqual([[{ block: "nearest" }]])
  expect(paintFindHighlights(root, "blue", { messageId: "unmounted", index: 0 })).toBeUndefined()
  expect(registry.get("drift-find")!.ranges).toHaveLength(1)
  expect(registry.has("drift-find-active")).toBe(false)
  expect(paintFindHighlights(root, "", undefined)).toBeUndefined()
  expect(registry.size).toBe(0)
})

test("chat gates disclosure navigation by query and occurrence identity rather than repaint or flat cursor", async () => {
  const ts = await import("typescript")
  const parsed = ts.createSourceFile("chat.tsx", await Bun.file("src/ui/chat.tsx").text(), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const calls: import("typescript").CallExpression[] = []
  function visit(node: import("typescript").Node) {
    if (ts.isCallExpression(node)) calls.push(node)
    ts.forEachChild(node, visit)
  }
  visit(parsed)
  const scroll = calls.filter((node) => node.expression.getText(parsed) === "scrollFindOccurrence")
  expect(scroll).toHaveLength(1)
  const guard = scroll[0].parent.parent.parent
  expect(ts.isIfStatement(guard)).toBe(true)
  if (!ts.isIfStatement(guard)) throw new Error("Navigation must be guarded")
  expect(guard.expression.getText(parsed)).toBe("active && target !== scrolledFindOccurrence")
  expect(guard.thenStatement.getText(parsed)).toContain("scrolledFindOccurrence = target")
  const identity = calls.find((node) => node.expression.getText(parsed) === "JSON.stringify" && node.arguments[0]?.getText(parsed).includes("occurrence.messageId"))!
  expect(identity.arguments[0].getText(parsed)).toBe("[value, occurrence.messageId, occurrence.index]")
  const paint = calls.find((node) => node.expression.getText(parsed) === "paintFindHighlights")!
  expect(paint.arguments.map((node) => node.getText(parsed))).toEqual(["scroller", "value", "occurrence"])
})
