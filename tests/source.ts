import { expect } from "bun:test"

// Collapses layout so a check on source text survives formatting: whitespace runs vanish next to punctuation and
// shrink to one space elsewhere, and a trailing comma before a closing bracket is dropped.
export function code(text: string) {
  const spaced = text.replace(/\s+/g, " ")
  const tight = spaced.replace(/ ?([(){}[\]<>,;:=|&?!+*/-]) ?/g, "$1")
  return tight.replace(/,([)\]}>])/g, "$1")
}

expect.extend({
  toContainCode(received: unknown, expected: string) {
    const pass = typeof received === "string" && code(received).includes(code(expected))
    return {
      pass,
      message: () => `expected source ${pass ? "not " : ""}to contain code:\n${expected}`,
    }
  },
})

declare module "bun:test" {
  interface Matchers<T> {
    toContainCode(expected: string): T
  }
}
