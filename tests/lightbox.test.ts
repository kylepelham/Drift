import { expect, test } from "bun:test"

test("wide lightbox images can always reach actual size", async () => {
  const { clampLightboxZoom } = await import("../src/ui/lightbox")
  expect(clampLightboxZoom(20, 0.05)).toBe(20)
  expect(clampLightboxZoom(30, 0.05)).toBe(20)
  expect(clampLightboxZoom(9, 0.5)).toBe(8)
  expect(clampLightboxZoom(0.01, 1)).toBe(0.1)
})

test("lightbox keeps wide content reachable and leaves plain wheel input for panning", async () => {
  const source = await Bun.file("src/ui/lightbox.tsx").text()
  expect(source).toContain('class="grid h-max min-h-full w-max min-w-full place-items-center p-8"')
  expect(source).toContain("if (!event.ctrlKey && !event.metaKey) return")
})
