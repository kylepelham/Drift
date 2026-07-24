import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { Portal } from "solid-js/web"
import { t } from "../state/i18n"
import { IconX } from "./icons"
import { activateModal, closeOnBackdropPointerDown } from "./modal"
import { McpManagement } from "./mcp/manager"

export { McpManagement } from "./mcp/manager"

const [open, setOpen] = createSignal(false)

export function openMcpServers() {
  setOpen(true)
}

export function McpServersModal() {
  return (
    <Show when={open()}>
      <Portal>
        <McpDialog onClose={() => setOpen(false)} />
      </Portal>
    </Show>
  )
}

function McpDialog(props: { onClose: () => void }) {
  let dialog!: HTMLDivElement
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))
  return (
    <div
      data-modal-layer
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/50 p-2 sm:p-4"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t("dialog.mcp.title")}
        tabIndex={-1}
        class="fade-up flex h-[min(44rem,calc(100vh-1rem))] w-[min(50rem,calc(100vw-1rem))] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40 sm:h-[min(44rem,calc(100vh-2rem))]"
      >
        <div class="flex items-start justify-between border-b border-edge px-4 py-3">
          <div>
            <div class="text-sm font-semibold text-ink">{t("dialog.mcp.title")}</div>
            <div class="mt-0.5 text-xs text-ink-faint">{t("drift.mcp.description")}</div>
          </div>
          <button
            title={t("common.close")}
            class="flex size-7 items-center justify-center rounded-md text-ink-faint hover:bg-raised hover:text-ink"
            onClick={props.onClose}
          >
            <IconX />
          </button>
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-4">
          <McpManagement />
        </div>
      </div>
    </div>
  )
}
