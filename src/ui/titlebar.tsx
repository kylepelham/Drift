import { Show, type JSX } from "solid-js"

type ShellWindow = { minimize(): void; toggleMaximize(): void; close(): void }
type ShellGlobal = { window?: { getCurrentWindow(): ShellWindow } }

function shellWindow() {
  return (globalThis as { __TAURI__?: ShellGlobal }).__TAURI__?.window?.getCurrentWindow()
}

export function Titlebar() {
  const controls = !!shellWindow() || import.meta.env.DEV
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
          <WindowButton label="Maximize" onClick={() => shellWindow()?.toggleMaximize()}>
            <rect x="4" y="4" width="8" height="8" rx="1" />
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
