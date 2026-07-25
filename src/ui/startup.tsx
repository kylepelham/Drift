import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js"
import type { Connection } from "../engine/connection"
import { useEngine } from "../engine"
import { pluginsSettled } from "../plugins"
import { t } from "../state/i18n"
import { activeWorkspace, workspacesReady } from "../state/workspaces"
import { DriftLogo } from "./logo"

export const maxSplashDuration = 3200
const splashExitDuration = 650

export function startupReady(input: {
  workspacesReady: boolean
  pluginsSettled: boolean
  workspacePath: string | null
  connection: Connection
  bootstrappedDirectory: string
  startupError: string
}) {
  if (input.startupError) return true
  if (!input.workspacesReady) return false
  if (!input.pluginsSettled) return false
  if (!input.workspacePath) return true
  return input.connection === "online" && input.bootstrappedDirectory === input.workspacePath
}

export function StartupSplash() {
  const engine = useEngine()
  const nativeTitlebar = Boolean((globalThis as { __TAURI__?: { window?: unknown } }).__TAURI__?.window)
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
        pluginsSettled: pluginsSettled(),
        workspacePath,
        connection: engine.state.connection,
        bootstrappedDirectory: engine.state.bootstrappedDirectory,
        startupError: engine.state.startupError,
      })
    )
      wash()
  })

  const status = () => {
    if (washing()) return t("startup.ready")
    if (!workspacesReady()) return t("startup.workspaces")
    if (!pluginsSettled()) return t("startup.plugins")
    const workspace = activeWorkspace()
    if (!workspace) return t("startup.ready")
    if (engine.state.connection !== "online") return t("startup.engine")
    if (engine.state.bootstrappedDirectory !== workspace.path) return t("startup.plugins")
    return t("startup.ready")
  }

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
        classList={{ "startup-splash-native": nativeTitlebar }}
        data-phase={washing() ? "washing" : "swimming"}
        role="status"
        aria-live="polite"
        aria-atomic="true"
        aria-label={status()}
        onAnimationEnd={(event) => {
          if (washing() && event.target === event.currentTarget) finish()
        }}
      >
        <div class="startup-backdrop" />
        <div class="startup-wash" aria-hidden="true">
          <span class="startup-wave startup-wave-back" />
          <span class="startup-wave startup-wave-middle" />
          <span class="startup-wave startup-wave-front" />
        </div>
        <div class="startup-scene" aria-hidden="true">
          <DriftLogo class="startup-jelly" />
          <div class="startup-copy">
            <div class="startup-name">Drift</div>
            <div class="startup-status">{status()}</div>
          </div>
        </div>
      </div>
    </Show>
  )
}
