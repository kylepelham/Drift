import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js"
import type { Connection } from "../engine/connection"
import { useEngine } from "../engine"
import { activeWorkspace, workspacesReady } from "../state/workspaces"
import { DriftLogo } from "./logo"

const infinityPath =
  "M 180 90 C 145 25 55 25 55 90 C 55 155 145 155 180 90 C 215 25 305 25 305 90 C 305 155 215 155 180 90"

export const maxSplashDuration = 3200
const splashExitDuration = 700

export function startupReady(input: {
  workspacesReady: boolean
  workspacePath: string | null
  connection: Connection
  bootstrappedDirectory: string
  startupError: string
}) {
  if (input.startupError) return true
  if (!input.workspacesReady) return false
  if (!input.workspacePath) return true
  return input.connection === "online" && input.bootstrappedDirectory === input.workspacePath
}

export function StartupSplash() {
  const engine = useEngine()
  const [visible, setVisible] = createSignal(true)
  const [washing, setWashing] = createSignal(false)
  let ceilingTimer: ReturnType<typeof setTimeout> | undefined
  let exitTimer: ReturnType<typeof setTimeout> | undefined

  const finish = () => {
    clearTimeout(exitTimer)
    setVisible(false)
  }
  const wash = () => {
    if (washing()) return
    clearTimeout(ceilingTimer)
    setWashing(true)
    exitTimer = setTimeout(finish, splashExitDuration)
  }

  createEffect(() => {
    const workspacePath = activeWorkspace()?.path ?? null
    if (
      startupReady({
        workspacesReady: workspacesReady(),
        workspacePath,
        connection: engine.state.connection,
        bootstrappedDirectory: engine.state.bootstrappedDirectory,
        startupError: engine.state.startupError,
      })
    )
      wash()
  })

  onMount(() => {
    ceilingTimer = setTimeout(wash, maxSplashDuration)
  })
  onCleanup(() => {
    clearTimeout(ceilingTimer)
    clearTimeout(exitTimer)
  })

  return (
    <Show when={visible()}>
      <div
        class="startup-splash"
        data-phase={washing() ? "washing" : "swimming"}
        data-tauri-drag-region=""
        role="status"
        aria-label="Starting Drift"
        onAnimationEnd={(event) => {
          if (washing() && event.target === event.currentTarget) finish()
        }}
      >
        <div class="startup-backdrop" data-tauri-drag-region="" />
        <div class="startup-wash" aria-hidden="true" />
        <div class="startup-scene" aria-hidden="true">
          <div class="startup-orbit">
            <svg class="startup-loop" viewBox="0 0 360 180">
              <defs>
                <linearGradient id="startup-current" x1="0" y1="0" x2="1" y2="1">
                  <stop offset="0" stop-color="#72c9ee" stop-opacity="0.12" />
                  <stop offset="0.55" stop-color="#a7e6ff" stop-opacity="0.9" />
                  <stop offset="1" stop-color="#72c9ee" stop-opacity="0.08" />
                </linearGradient>
              </defs>
              <path class="startup-loop-base" d={infinityPath} />
              <path class="startup-loop-glow" d={infinityPath} />
              <path class="startup-loop-current" d={infinityPath} />
            </svg>
            <div class="startup-swimmer">
              <span class="startup-bubble startup-bubble-one" />
              <span class="startup-bubble startup-bubble-two" />
              <DriftLogo class="startup-jelly" />
            </div>
          </div>
          <div class="startup-name">Drift</div>
        </div>
      </div>
    </Show>
  )
}
