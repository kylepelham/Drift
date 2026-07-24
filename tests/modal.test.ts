import { expect, test } from "bun:test"
import { closeOnBackdropPointerDown } from "../src/ui/modal"

test("modal backdrop closes only when the pointer starts outside the dialog", () => {
  const backdrop = {}
  const dialog = {}
  let closes = 0
  const close = () => closes++

  closeOnBackdropPointerDown({ target: dialog, currentTarget: backdrop }, close)
  expect(closes).toBe(0)

  closeOnBackdropPointerDown({ target: backdrop, currentTarget: backdrop }, close)
  expect(closes).toBe(1)
})
