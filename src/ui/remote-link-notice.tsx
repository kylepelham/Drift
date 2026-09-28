import { createSignal, For, onCleanup, onMount } from "solid-js"
import { isRemoteRuntime } from "../runtime"
import { t } from "../state/i18n"
import { listenRemoteAccess, refreshRemoteAccess, remoteAccessStatus, type PendingLink } from "../state/remote-access"
import { openSettings } from "./settings"

const key = (link: PendingLink) => `${link.address}:${link.requestedAt}`

/** Prompts on the desktop when a device is showing a link code, wherever the user is in Drift. */
export function RemoteLinkNotice() {
  const [dismissed, setDismissed] = createSignal<ReadonlySet<string>>(new Set())
  onMount(() => {
    if (isRemoteRuntime()) return
    void refreshRemoteAccess()
    const stop = listenRemoteAccess()
    // Codes expire without an event, so refresh while any are outstanding.
    const timer = setInterval(() => remoteAccessStatus()?.pendingLinks.length && void refreshRemoteAccess(), 30_000)
    onCleanup(() => {
      clearInterval(timer)
      void stop?.then((release) => release())
    })
  })
  const visible = () => (remoteAccessStatus()?.pendingLinks ?? []).filter((link) => !dismissed().has(key(link)))
  const dismiss = (link: PendingLink) => setDismissed((current) => new Set([...current, key(link)]))

  return (
    <For each={visible()}>
      {(link) => (
        <div class="rounded-lg border border-warn/40 bg-surface/95 px-3 py-2 shadow-xl backdrop-blur" role="status">
          <div class="text-sm font-semibold text-ink">{t("drift.remote.toast.title")}</div>
          <div class="mt-0.5 text-sm text-ink">{t("drift.remote.toast.message", { name: `${link.name} (${link.address})` })}</div>
          <div class="mt-2 flex flex-wrap gap-1.5">
            <button
              class="rounded-md border border-accent/40 px-2 py-1 text-xs text-accent hover:bg-accent/10"
              onClick={() => {
                dismiss(link)
                openSettings("Remote Access")
              }}
            >
              {t("drift.remote.toast.open")}
            </button>
            <button class="rounded-md border border-edge px-2 py-1 text-xs text-ink-muted hover:text-ink" onClick={() => dismiss(link)}>
              {t("common.dismiss")}
            </button>
          </div>
        </div>
      )}
    </For>
  )
}
