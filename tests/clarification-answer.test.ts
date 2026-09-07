import { expect, test } from "bun:test"
import type { MessageEntry } from "../src/engine/store"
import { messageText } from "../src/engine/store"
import { clarificationAnswer } from "../src/ui/clarification-answer"
import { entrySearchText, transcriptMatches } from "../src/state/transcript-search"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const requestID = "que_A1b2C3d4"
const single = { header: "Color", question: "Which color?", answers: ["Blue"] }
const items = [
  { header: "Colors", question: "Which colors?", answers: ["Blue", "Green, teal"] },
  { header: "Notes", question: "Any notes?\nInclude details.", answers: ["  Keep raw\n\ntext <unchanged>\r\nwith spaces  "] },
  { header: "Missing", question: "Anything else?", answers: [] },
]
const legacyBody = "Which colors?\nBlue, Green, teal\n\nAny notes?\nKeep raw\ntext <unchanged>\n\nAnything else?\nUnanswered"
const legacyText = `Answer to clarification ${requestID}:\n${legacyBody}`

function entry(text = legacyText, metadata?: Record<string, unknown>): MessageEntry {
  return {
    info: {
      id: "u1", sessionID: "s1", role: "user", time: { created: 1 },
      agent: "plan", model: { providerID: "test", modelID: "test" },
    },
    parts: [{ id: "p1", messageID: "u1", sessionID: "s1", type: "text", text, ...(metadata ? { metadata } : {}) }],
  }
}

function structured(rows = items) {
  return entry(legacyText, { driftClarification: { version: 1, requestID, items: rows } })
}

test("metadata search uses rendered questions and answers, not protocol, headers, or empty-answer UI", () => {
  const message = structured()
  expect(entrySearchText(message)).toBe(clarificationAnswer(message)!.text)
  for (const query of ["which colors?", "include details.", "Green, teal", "text <unchanged>", "anything else?"])
    expect(transcriptMatches([message], query)).toEqual([{ messageId: "u1", count: 1 }])
  for (const query of [requestID, "Answer to clarification", "Missing", "Unanswered"])
    expect(transcriptMatches([message], query)).toEqual([])
  const repeated = structured([{ ...single, question: "Blue or not Blue?", answers: ["Blue", "Blue"] }])
  expect(transcriptMatches([repeated], "blue")).toEqual([{ messageId: "u1", count: 4 }])
})

test("legacy search preserves the full displayed body but excludes the protocol prefix", () => {
  const message = entry()
  expect(entrySearchText(message)).toBe(legacyBody)
  for (const query of ["Which colors?", "Blue", "Unanswered"])
    expect(transcriptMatches([message], query)).toEqual([{ messageId: "u1", count: 1 }])
  for (const query of [requestID, "Answer to clarification"])
    expect(transcriptMatches([message], query)).toEqual([])
})

test("clarification search cache follows metadata changes even when raw text and answer lengths stay the same", () => {
  const row = { ...single, answers: ["Blue"] }
  const message = structured([row])
  expect(transcriptMatches([message], "blue")).toEqual([{ messageId: "u1", count: 1 }])
  row.answers[0] = "Teal"
  expect(transcriptMatches([message], "blue")).toEqual([])
  expect(transcriptMatches([message], "teal")).toEqual([{ messageId: "u1", count: 1 }])
})

test("ordinary and malformed clarification messages still search their raw displayed text", () => {
  for (const message of [
    entry("An ordinary Answer to clarification question"),
    entry(legacyText, { driftClarification: { version: 2, requestID, items } }),
    { ...entry(), parts: [...entry().parts, ...entry("attachment text").parts] },
    { ...entry(), info: { ...entry().info, role: "assistant" } } as MessageEntry,
  ]) expect(transcriptMatches([message], "Answer to clarification")).toEqual([{ messageId: "u1", count: 1 }])
})

test("exact driftClarification version 1 metadata renders a single question without protocol text", () => {
  expect(clarificationAnswer(structured([single]))).toEqual({
    items: [single], text: "Which color?\nBlue", preview: "Blue",
  })
  expect(clarificationAnswer(entry("ordinary message", {
    driftClarification: { version: 1, requestID, items: [single] },
  }))).toEqual(clarificationAnswer(structured([single])))
})

test("multiple questions preserve multiselect boundaries, custom multiline text, and empty answers", () => {
  const result = clarificationAnswer(structured())!
  expect(result).toEqual({
    items,
    text: "Which colors?\nBlue, Green, teal\n\nAny notes?\nInclude details.\n  Keep raw\n\ntext <unchanged>\r\nwith spaces  \n\nAnything else?\n",
    preview: "Blue, Green, teal,   Keep raw\n\ntext <unchanged>\r\nwith spaces  ",
  })
  expect(result.text).not.toContain(requestID)
  expect(result.preview).not.toContain(requestID)
  expect(result.text).not.toContain("Answer to clarification")
  expect(result.items[0].answers).toEqual(["Blue", "Green, teal"])
})

test.each([{ answers: [] }, { answers: [""] }, { answers: ["", "  ", "\n"] }])("empty answers are retained, not dropped or invented: %j", ({ answers }) => {
  const item = { header: "", question: "Optional?", answers }
  expect(clarificationAnswer(structured([item]))).toEqual({
    items: [item], text: `Optional?\n${answers.join(", ")}`, preview: answers.join(", "),
  })
})

test("metadata survives a persistence-like JSON roundtrip without mutating stored text or answers", () => {
  const persisted = JSON.stringify(structured())
  const restored: MessageEntry = JSON.parse(persisted)
  const before = clarificationAnswer(restored)!
  expect(before).toEqual(clarificationAnswer(structured()))
  expect(JSON.stringify(restored)).toBe(persisted)
  expect(messageText(restored)).toBe(legacyText)

  before.items[0].answers.push("Changed outside the message")
  before.items[1].answers[0] = "Replaced"
  before.items[0].question = "Changed question"
  before.items[0].header = "Changed header"
  before.items.pop()
  expect(JSON.stringify(restored)).toBe(persisted)
  expect(clarificationAnswer(restored)).toEqual(clarificationAnswer(structured()))
})

test.each([
  ["null", null],
  ["boolean", true],
  ["string", "version 1"],
  ["array", []],
  ["missing version", { requestID, items: [single] }],
  ["string version", { version: "1", requestID, items: [single] }],
  ["unknown version", { version: 2, requestID, items: [single] }],
  ["missing requestID", { version: 1, items: [single] }],
  ["non-string requestID", { version: 1, requestID: 42, items: [single] }],
  ["missing items", { version: 1, requestID }],
  ["non-array items", { version: 1, requestID, items: {} }],
  ["empty items", { version: 1, requestID, items: [] }],
  ...[
    null, "question", {}, { ...single, header: undefined }, { ...single, header: 1 },
    { ...single, question: null }, { ...single, question: undefined },
    { ...single, answers: undefined }, { ...single, answers: "Blue" },
    { ...single, answers: ["Blue", null] }, { ...single, answers: [42] },
  ].map((item, index) => [`invalid item ${index}`, { version: 1, requestID, items: [single, item] }]),
])("malformed metadata (%s) falls back to raw text even with a valid legacy prefix", (_, metadata) => {
  const message = entry(legacyText, { driftClarification: metadata })
  const before = JSON.stringify(message)
  expect(clarificationAnswer(message)).toBeUndefined()
  expect(messageText(message)).toBe(legacyText)
  expect(JSON.stringify(message)).toBe(before)
})

test("other metadata names and nested lookalikes do not opt ordinary messages into collapsing", () => {
  const data = { version: 1, requestID, items: [single] }
  for (const metadata of [{ clarification: data }, { driftclarification: data }, { other: { driftClarification: data } }])
    expect(clarificationAnswer(entry("ordinary message", metadata))).toBeUndefined()
})

test.each(["metadata", "legacy"])("%s detection excludes assistant, synthetic, multipart, and attachment messages", (format) => {
  const message = format === "metadata" ? structured() : entry()
  const part = message.parts[0]
  const file = { id: "f1", messageID: "u1", sessionID: "s1", type: "file", mime: "image/png", url: "file:///image.png" } as const
  const excluded = [
    { ...message, info: { ...message.info, role: "assistant" } } as MessageEntry,
    { ...message, parts: [{ ...part, synthetic: true }] } as MessageEntry,
    { ...message, parts: [part, { ...part, id: "p2" }] },
    { ...message, parts: [part, file] },
    { ...message, parts: [file, part] },
    { ...message, parts: [file] },
    { ...message, parts: [] },
    { ...message, parts: [{ ...part, type: "reasoning" }] } as MessageEntry,
  ]
  for (const candidate of excluded) expect(clarificationAnswer(candidate)).toBeUndefined()
  expect(clarificationAnswer(entry("Please keep this ordinary user message expanded."))).toBeUndefined()
})

test.each(["\n", "\r\n"])("legacy que_ID protocol recognizes persisted replies with %j and preserves the entire body", (newline) => {
  const body = `${legacyBody}${newline}\nA custom answer?\nYes: keep this colon\n\nAnswer to clarification que_Embedded:\nDo not strip this\n  `
  const message = entry(`Answer to clarification ${requestID}:${newline}${body}`)
  const persisted = JSON.stringify(message)
  expect(clarificationAnswer(JSON.parse(persisted))).toEqual({ items: [], text: body, preview: "" })
  expect(JSON.stringify(message)).toBe(persisted)
})

test.each([
  "Answer to clarification que_X:\nQ?\nA",
  "Answer to clarification que_0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ:\nQ?\nA",
])("legacy IDs accept only the persisted alphanumeric identifier shape: %s", (text) => {
  expect(clarificationAnswer(entry(text))).toEqual({ items: [], text: "Q?\nA", preview: "" })
})

test.each([
  "Answer to clarification que_:\nQ?\nA",
  "Answer to clarification q_ABC:\nQ?\nA",
  "Answer to clarification que_A-B:\nQ?\nA",
  "Answer to clarification que_A_B:\nQ?\nA",
  "Answer to clarification que_ABC: Q?\nA",
  "Answer to clarification que_ABC:\rQ?\nA",
  "Answer to clarification que_ABC:\n",
  "Answer to clarification que_ABC: \nQ?\nA",
  "answer to clarification que_ABC:\nQ?\nA",
  " Answer to clarification que_ABC:\nQ?\nA",
  "Some prose\nAnswer to clarification que_ABC:\nQ?\nA",
  "```\nAnswer to clarification que_ABC:\nQ?\nA\n```",
])("legacy lookalikes remain ordinary raw messages: %s", (text) => {
  const message = entry(text)
  expect(clarificationAnswer(message)).toBeUndefined()
  expect(messageText(message)).toBe(text)
})

test("old and new clarification rows estimate 40px regardless of answer length or font size", async () => {
  const { estimatedTimelineRow } = await import("../src/ui/chat")
  const long = "A long custom answer\n".repeat(100)
  for (const message of [entry(), structured(), entry(`Answer to clarification ${requestID}:\n${long}`), structured([{ ...single, answers: [long] }])]) {
    for (const fontSize of [13, 16, 24]) {
      expect(estimatedTimelineRow(message, fontSize)).toBe(40)
      expect(estimatedTimelineRow(message, fontSize, [...message.parts])).toBe(40)
    }
  }
})

test("normal estimates and explicit thinking/summary modes are unaffected", async () => {
  const { estimatedTimelineRow } = await import("../src/ui/chat")
  expect(estimatedTimelineRow(entry("Short ordinary message"))).toBe(96)
  const long = entry(Array.from({ length: 41 }, () => "line").join("\n"))
  expect(estimatedTimelineRow(long)).toBe(915)
  expect(estimatedTimelineRow(long, 16)).toBe(1112)
  const message = structured()
  expect(estimatedTimelineRow(message, 13, message.parts, true)).toBe(32)
  expect(estimatedTimelineRow(message, 13, message.parts, false, true)).toBe(44)
  expect(estimatedTimelineRow(message, 13, entry("ordinary override").parts)).toBe(96)
  expect(estimatedTimelineRow(entry("ordinary original"), 13, message.parts)).toBe(40)
  for (const candidate of [
    entry(legacyText, { driftClarification: { version: 2, requestID, items } }),
    { ...message, info: { ...message.info, role: "assistant" } } as MessageEntry,
    { ...message, parts: [{ ...message.parts[0], synthetic: true }] } as MessageEntry,
    { ...message, parts: [...message.parts, ...entry("attachment text").parts] },
    { ...message, parts: [...message.parts, { id: "f1", messageID: "u1", sessionID: "s1", type: "file", mime: "image/png", url: "file:///image.png" }] } as MessageEntry,
  ]) expect(estimatedTimelineRow(candidate)).toBeGreaterThanOrEqual(96)
})

test("clarification UI uses a closed native disclosure with full QAs, copy/revert, and no normal bubble footer", async () => {
  const ts = await import("typescript")
  const source = await Bun.file("src/ui/message.tsx").text()
  const parsed = ts.createSourceFile("message.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  const userBubble = parsed.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "UserBubble")!
  expect(userBubble).toBeDefined()
  const elements: import("typescript").JsxElement[] = []
  function visit(node: import("typescript").Node) {
    if (ts.isJsxElement(node)) elements.push(node)
    ts.forEachChild(node, visit)
  }
  visit(userBubble)
  const branch = elements.find((node) => node.openingElement.tagName.getText(parsed) === "Show" &&
    node.openingElement.attributes.properties.some((attr) => ts.isJsxAttribute(attr) &&
      attr.name.getText(parsed) === "when" && attr.initializer?.getText(parsed) === "{clarification()}"))!
  expect(branch).toBeDefined()
  const fallback = branch.openingElement.attributes.properties.find((attr) => ts.isJsxAttribute(attr) && attr.name.getText(parsed) === "fallback")!
  expect(fallback.getText(parsed)).toContain("bg-surface")
  expect(fallback.getText(parsed)).toContain("model()")

  elements.length = 0
  branch.children.forEach(visit)
  const details = elements.filter((node) => node.openingElement.tagName.getText(parsed) === "details")
  expect(details).toHaveLength(1)
  expect(details[0].openingElement.attributes.properties.some((attr) => ts.isJsxAttribute(attr) && attr.name.getText(parsed) === "open")).toBe(false)
  const summary = elements.find((node) => node.openingElement.tagName.getText(parsed) === "summary")!
  expect(summary).toBeDefined()
  expect(summary.parent).toBe(details[0])
  expect(summary.getText(parsed)).toContain('t("drift.question.answered")')
  expect(summary.getText(parsed)).toContain("answer().preview")
  expect(summary.openingElement.attributes.properties.some((attr) => ts.isJsxAttribute(attr) && attr.name.getText(parsed) === "data-find-ignore")).toBe(true)
  const ignored = elements.flatMap((node) => [...node.openingElement.attributes.properties])
    .filter((attr) => ts.isJsxAttribute(attr) && attr.name.getText(parsed) === "data-find-ignore")
  expect(ignored.map((attr) => attr.getText(parsed))).toEqual([
    "data-find-ignore", 'data-find-ignore={item.answers.length ? undefined : ""}',
  ])
  expect(fallback.getText(parsed)).not.toContain("data-find-ignore")

  const expanded = details[0].children.filter((node) => node !== summary).map((node) => node.getText(parsed)).join("\n")
  expect(expanded).toContain("<For each={answer().items}>")
  expect(expanded).toContain("{item.question}")
  expect(expanded).toContain('item.answers.join(", ")')
  expect(expanded).toContain('t("drift.question.unanswered")')
  expect(expanded).toContain("{answer().text}")
  expect(expanded).toContain("whitespace-pre-wrap")
  expect(expanded).not.toMatch(/\btruncate\b|line-clamp|max-h-|\.slice\(/)

  const special = branch.children.map((node) => node.getText(parsed)).join("\n")
  expect(special).toContain('title={t("drift.message.copy")}')
  expect(special).toContain("navigator.clipboard.writeText(text())")
  expect(special).toContain('title={t("drift.message.revertHere")}')
  expect(special).toContain("onClick={() => void revert()}")
  expect(special).toContain("group-focus-within:opacity-100")
  expect(special).not.toMatch(/bg-surface|<Markdown|<AssistantFlow|\bmodel\(\)|\btime\(\)|\bagentLabel\(/)
  expect(userBubble.getText(parsed)).toContain("clarification()?.text ?? messageText(props.entry)")
  expect(userBubble.getText(parsed)).toContain("engine.actions.revert(info().sessionID, info().id)")
  expect(userBubble.getText(parsed)).toContain("if (clarification()) restored.text = text()")
})
