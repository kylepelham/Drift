import { createSignal, onCleanup, onMount, Show, type JSX } from "solid-js"

type ShellWindow = {
  minimize(): void
  toggleMaximize(): void
  close(): void
  isMaximized(): Promise<boolean>
  onResized(handler: () => void): Promise<() => void>
}
type ShellGlobal = { window?: { getCurrentWindow(): ShellWindow } }

function shellWindow() {
  return (globalThis as { __TAURI__?: ShellGlobal }).__TAURI__?.window?.getCurrentWindow()
}

export function Titlebar() {
  const controls = !!shellWindow() || import.meta.env.DEV
  const [maximized, setMaximized] = createSignal(false)

  onMount(() => {
    const shell = shellWindow()
    if (!shell) return
    const refresh = () => void shell.isMaximized().then(setMaximized).catch(() => {})
    refresh()
    void shell
      .onResized(refresh)
      .then((dispose) => onCleanup(dispose))
      .catch(() => {})
  })

  return (
    <header
      data-tauri-drag-region
      class="flex h-9 shrink-0 items-center justify-between border-b border-edge bg-surface select-none"
    >
      <span class="pointer-events-none px-4 text-[0.7rem] font-semibold tracking-[0.2em] text-ink-muted">DRIFT</span>
      <Show when={controls}>
        <div class="flex h-full">
          <WindowButton label="Minimize" onClick={() => shellWindow()?.minimize()}>
            <path d="M3 8h10" />
          </WindowButton>
          <WindowButton
            label={maximized() ? "Restore" : "Maximize"}
            onClick={() => shellWindow()?.toggleMaximize()}
          >
            <Show when={maximized()} fallback={<rect x="4" y="4" width="8" height="8" rx="1" />}>
              <>
                <rect x="3" y="5.5" width="7.5" height="7.5" rx="1" />
                <path d="M5.5 5.5V4a1 1 0 0 1 1-1H12a1 1 0 0 1 1 1v5.5a1 1 0 0 1-1 1h-1.5" />
              </>
            </Show>
          </WindowButton>
          <WindowButton label="Close" danger onClick={() => shellWindow()?.close()}>
            <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
          </WindowButton>
        </div>
      </Show>
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
