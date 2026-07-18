import { createEffect } from "solid-js"
import { persisted } from "./persist"

export const themes = ["drift-dark", "drift-slate", "drift-light"] as const
export type ThemeName = (typeof themes)[number]

export const [theme, setTheme] = persisted<ThemeName>("drift.theme", "drift-dark")

export function bindTheme() {
  createEffect(() => {
    document.documentElement.dataset.theme = theme()
  })
}
