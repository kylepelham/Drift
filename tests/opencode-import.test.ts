import { acceptImportProgress, opencodeImport } from "../src/state/opencode-import"
import { expect, test } from "bun:test"

test("import progress shows while conversations are still coming and clears once the last is in", () => {
  acceptImportProgress({ done: 0, total: 1479 })
  expect(opencodeImport()).toEqual({ done: 0, total: 1479 })
  acceptImportProgress({ done: 312, total: 1479 })
  expect(opencodeImport()).toEqual({ done: 312, total: 1479 })
  acceptImportProgress({ done: 1479, total: 1479 })
  expect(opencodeImport()).toBeNull()
  acceptImportProgress({ done: 0, total: 0 })
  expect(opencodeImport()).toBeNull()
})
