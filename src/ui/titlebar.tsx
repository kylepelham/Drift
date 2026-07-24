import { createSignal, onCleanup, onMount, Show, type JSX } from "solid-js"
import { t } from "../state/i18n"
import { autoUpdate } from "../state/prefs"
import appIcon from "../../src-tauri/icons/32x32.png"

type ShellWindow = {
  minimize(): void
  toggleMaximize(): void
  close(): void
  isMaximized(): Promise<boolean>
  onResized(handler: () => void): Promise<() => void>
}
type ShellGlobal = {
  window?: { getCurrentWindow(): ShellWindow }
  core?: { invoke(command: string): Promise<unknown> }
}

function shellWindow() {
  return (globalThis as { __TAURI__?: ShellGlobal }).__TAURI__?.window?.getCurrentWindow()
}

function shellInvoke() {
  return (globalThis as { __TAURI__?: ShellGlobal }).__TAURI__?.core?.invoke
}

export function Titlebar() {
  const shell = shellWindow()
  if (!shell) return null
  const [maximized, setMaximized] = createSignal(false)
  const [update, setUpdate] = createSignal<string | null>(null)
  const [installing, setInstalling] = createSignal(false)

  onMount(() => {
    const refresh = () => void shell.isMaximized().then(setMaximized).catch(() => {})
    refresh()
    void shell
      .onResized(refresh)
      .then((dispose) => onCleanup(dispose))
      .catch(() => {})
    const check = () => {
      if (!autoUpdate() || update()) return
      void shellInvoke()?.("check_update")
        .then((version) => typeof version === "string" && setUpdate(version))
        .catch(() => {})
    }
    check()
    const timer = setInterval(check, 4 * 60 * 60 * 1000)
    onCleanup(() => clearInterval(timer))
  })

  const install = () => {
    setInstalling(true)
    void shellInvoke()?.("install_update").catch(() => setInstalling(false))
  }

  return (
    <header
      data-tauri-drag-region
      class="flex h-9 shrink-0 items-center justify-between border-b border-edge bg-surface select-none"
    >
      <div class="pointer-events-none flex items-center gap-2 px-3.5">
        <img src={appIcon} alt="" class="size-[18px]" />
        <span class="drift-wordmark">drift</span>
      </div>
      <div class="flex h-full items-center">
        <Show when={update()}>
          {(version) => (
            <button
              class="mr-2 flex h-6 items-center rounded-full border border-accent/40 bg-accent/10 px-2.5 text-[0.68rem] font-medium text-accent transition-colors hover:bg-accent/20 disabled:opacity-60"
              disabled={installing()}
              onClick={install}
            >
              {installing() ? t("settings.updates.action.installing") : t("error.page.action.updateTo", { version: version() })}
            </button>
          )}
        </Show>
          <div class="flex h-full">
            <WindowButton label={t("drift.titlebar.minimize")} onClick={() => shell.minimize()}>
              <path d="M3 8h10" />
            </WindowButton>
            <WindowButton
              label={maximized() ? t("drift.titlebar.restore") : t("drift.titlebar.maximize")}
              onClick={() => shell.toggleMaximize()}
            >
              <Show when={maximized()} fallback={<rect x="4" y="4" width="8" height="8" rx="1" />}>
                <>
                  <rect x="3" y="5.5" width="7.5" height="7.5" rx="1" />
                  <path d="M5.5 5.5V4a1 1 0 0 1 1-1H12a1 1 0 0 1 1 1v5.5a1 1 0 0 1-1 1h-1.5" />
                </>
              </Show>
            </WindowButton>
            <WindowButton label={t("common.close")} danger onClick={() => shell.close()}>
              <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
            </WindowButton>
          </div>
      </div>
    </header>
  )
}

function WindowButton(props: { label: string; danger?: boolean; onClick: () => void; children: JSX.Element }) {
  return (
    <button
      title={props.label}
      class="flex h-full w-11 items-center justify-center text-ink-faint transition-colors"
      classList={{ "hover:bg-danger hover:text-white": props.danger, "hover:bg-raised hover:text-ink": !props.danger }}
      onClick={props.onClick}
    >
      <svg class="size-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2">
        {props.children}
      </svg>
    </button>
  )
}
