import { expect, test } from "bun:test"
import { generate, output } from "../scripts/gen-engine-client"

test("src/engine/native/types.ts matches the engine's OpenAPI", async () => {
  const committed = await Bun.file(output).text()
  expect(committed).toBe(await generate())
}, 300_000)
