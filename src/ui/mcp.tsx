import type { McpStatus } from "@opencode-ai/sdk/client"
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import { t } from "../state/i18n"
import { IconX } from "./icons"
import { closeOnBackdropPointerDown } from "./modal"
import { Toggle } from "./model-manager"

const [open, setOpen] = createSignal(false)

export function openMcpServers() {
  setOpen(true)
}

export function McpServersModal() {
  return (
    <Show when={open()}>
      <McpDialog onClose={() => setOpen(false)} />
    </Show>
  )
}

function McpDialog(props: { onClose: () => void }) {
  const engine = useEngine()
  const [servers, setServers] = createSignal<Record<string, McpStatus>>({})
  const [loaded, setLoaded] = createSignal(false)
  const [pending, setPending] = createSignal<Record<string, boolean>>({})

  const refresh = async () => {
    setServers(await engine.actions.mcpStatus())
    setLoaded(true)
  }
  onMount(() => void refresh())

  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  const toggle = async (name: string, status: McpStatus) => {
    if (pending()[name]) return
    setPending({ ...pending(), [name]: true })
    if (status.status === "connected") await engine.actions.mcpDisconnect(name)
    else if (status.status === "needs_auth" || status.status === "needs_client_registration")
      await engine.actions.mcpAuthenticate(name)
    else await engine.actions.mcpConnect(name)
    await refresh()
    setPending({ ...pending(), [name]: false })
  }

  const label = (name: string, status: McpStatus) => {
    if (pending()[name])
      return {
        text: t(status.status === "connected" ? "drift.mcp.status.disconnecting" : "drift.mcp.status.connecting"),
        tone: "text-ink-faint",
      }
    switch (status.status) {
      case "connected":
        return { text: t("mcp.status.connected"), tone: "text-ok" }
      case "disabled":
        return { text: t("mcp.status.disabled"), tone: "text-ink-faint" }
      case "failed":
        return { text: t("mcp.status.failed"), tone: "text-danger" }
      case "needs_auth":
        return { text: t("mcp.status.needs_auth"), tone: "text-warn" }
      default:
        return { text: t("mcp.status.needs_client_registration"), tone: "text-warn" }
    }
  }

  return (
    <div
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose)}
    >
      <div
        class="fade-up flex max-h-[70vh] w-[34rem] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="flex items-start justify-between border-b border-edge px-4 py-3">
          <div>
            <div class="text-sm font-semibold text-ink">{t("dialog.mcp.title")}</div>
            <div class="mt-0.5 text-xs text-ink-faint">{t("drift.mcp.description")}</div>
          </div>
          <button
            title={t("common.close")}
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-2">
          <For each={Object.entries(servers())}>
            {([name, status]) => (
              <div
                class="cursor-pointer rounded-lg px-3 py-2 hover:bg-raised/60"
                onClick={(event) => {
                  if ((event.target as HTMLElement).closest("[data-error]")) return
                  void toggle(name, status)
                }}
              >
                <div class="flex items-center gap-2.5">
                  <span class="min-w-0 flex-1 truncate text-sm text-ink">{name}</span>
                  <span class={`shrink-0 text-xs ${label(name, status).tone}`} classList={{ "pulse-soft": !!pending()[name] }}>
                    {label(name, status).text}
                  </span>
                  <Toggle
                    label={t("drift.mcp.enable", { name })}
                    checked={status.status === "connected"}
                    disabled={!!pending()[name]}
                    onChange={() => void toggle(name, status)}
                  />
                </div>
                <Show when={!pending()[name] && "error" in status && status.error}>
                  {(error) => (
                    <div data-error class="mt-1 cursor-auto text-xs break-all text-danger select-text">
                      {error()}
                    </div>
                  )}
                </Show>
              </div>
            )}
          </For>
          <Show when={loaded() && Object.keys(servers()).length === 0}>
            <div class="px-3 py-4 text-sm text-ink-faint">{t("dialog.mcp.empty")}</div>
          </Show>
        </div>
      </div>
    </div>
  )
}
