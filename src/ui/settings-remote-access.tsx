import { createSignal, For, onCleanup, onMount, Show } from "solid-js"
import { isRemoteRuntime } from "../runtime"
import { t } from "../state/i18n"
import {
  linkRemoteDevice,
  listenRemoteAccess,
  loadRemoteSession,
  nextRemoteAccessEnabled,
  normalizeLinkCode,
  refreshRemoteAccess,
  remoteAccessBusy,
  remoteAccessError,
  remoteAccessStatus,
  remoteSession,
  remoteStatusTone,
  revokeRemoteDevice,
  setRemoteAccess,
  setRemotePassword,
  signOutRemoteDevice,
  type RemoteDevice,
} from "../state/remote-access"
import { Toggle } from "./controls"
import { SettingsGroup, SettingsRow } from "./settings-controls"

const button =
  "rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
const field =
  "w-full rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-ink outline-none focus:border-edge-strong sm:w-56"

export function RemoteAccessSection() {
  return (
    <div class="space-y-5">
      <Show when={!isRemoteRuntime()} fallback={<ThisDevice />}>
        <Gateway />
        <Show when={remoteAccessStatus()?.enabled}>
          <LinkDevice />
          <LinkedDevices />
          <PasswordSignIn />
          <Encryption />
        </Show>
      </Show>
      <Show when={remoteAccessError() || remoteAccessStatus()?.error}>
        <div class="text-xs text-danger">{remoteAccessError() || remoteAccessStatus()?.error}</div>
      </Show>
      <p class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.remote.securityNote")}</p>
    </div>
  )
}

function ThisDevice() {
  onMount(() => void loadRemoteSession().catch(() => undefined))
  return (
    <SettingsGroup title={t("drift.remote.device.title")}>
      <SettingsRow
        title={remoteSession() ? t("drift.remote.device.signedIn", { name: remoteSession()!.name }) : t("drift.remote.connected")}
        description={t("drift.remote.manageOnDesktop")}
      >
        <button class={button} onClick={() => void signOutRemoteDevice()}>{t("drift.remote.device.signOut")}</button>
      </SettingsRow>
    </SettingsGroup>
  )
}

function Gateway() {
  const status = remoteAccessStatus
  const [copied, setCopied] = createSignal(false)
  const [clipboardError, setClipboardError] = createSignal("")
  onMount(() => {
    void refreshRemoteAccess()
    const stop = listenRemoteAccess()
    onCleanup(() => void stop?.then((release) => release()))
  })

  async function copyAddress() {
    const url = status()?.urls[0]
    if (!url) return
    setClipboardError("")
    try {
      await navigator.clipboard.writeText(url)
      setCopied(true)
      setTimeout(() => setCopied(false), 1800)
    } catch (cause) {
      setClipboardError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const statusLabel = () =>
    status()?.error
      ? t("drift.remote.statusError")
      : status()?.listening
        ? t("drift.remote.listening")
        : status()?.enabled
          ? t("drift.remote.statusStarting")
          : t("drift.remote.statusOff")

  return (
    <>
      <SettingsGroup title={t("drift.remote.gateway")}>
        <SettingsRow title={t("drift.remote.enable")} description={t("drift.remote.enableDescription")}>
          <Toggle
            label={t("drift.remote.enable")}
            checked={!!status()?.enabled}
            disabled={remoteAccessBusy()}
            onChange={() => void setRemoteAccess(nextRemoteAccessEnabled(status()))}
          />
        </SettingsRow>
        <SettingsRow title={t("drift.remote.address")} description={statusLabel()}>
          <div class="flex items-center gap-2">
            <span
              class="size-2 rounded-full"
              classList={{
                "bg-ink-faint": remoteStatusTone(status()) === "idle" || (remoteStatusTone(status()) === "offline" && !status()?.enabled),
                "bg-warn": remoteStatusTone(status()) === "offline" && !!status()?.enabled,
                "bg-ok": remoteStatusTone(status()) === "online",
                "bg-danger": remoteStatusTone(status()) === "error",
              }}
            />
            <span class="font-mono text-[0.75rem] text-ink-muted">{status()?.listeningAddress ?? "-"}</span>
          </div>
        </SettingsRow>
        <Show when={status()?.enabled && status()?.listening}>
          <SettingsRow title={t("drift.remote.open.title")} description={t("drift.remote.open.description")}>
            <div class="flex flex-col items-end gap-2">
              <span class="font-mono text-[0.75rem] text-ink select-all">{status()?.urls[0] || t("drift.remote.noLanAddress")}</span>
              <Show when={status()?.urls[0]}>
                <button class={button} onClick={() => void copyAddress()}>
                  {copied() ? t("drift.remote.copied") : t("drift.remote.copy")}
                </button>
              </Show>
            </div>
          </SettingsRow>
          <Show when={status()?.addressQr}>
            {(svg) => (
              <div class="flex items-center gap-4 px-1 py-3">
                <div class="size-36 shrink-0 rounded-md bg-white p-1.5 [&>svg]:size-full" role="img" aria-label={t("drift.remote.scan")} innerHTML={svg()} />
                <p class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.remote.scan")}</p>
              </div>
            )}
          </Show>
        </Show>
      </SettingsGroup>
      <Show when={clipboardError()}><div class="text-xs text-danger">{t("drift.remote.clipboardError")}: {clipboardError()}</div></Show>
    </>
  )
}

function LinkDevice() {
  const [code, setCode] = createSignal("")
  const [linked, setLinked] = createSignal("")
  const [error, setError] = createSignal("")
  const [working, setWorking] = createSignal(false)
  const pending = () => remoteAccessStatus()?.pendingLinks ?? []
  const complete = () => normalizeLinkCode(code()).length === 8

  async function submit(event: Event) {
    event.preventDefault()
    if (!complete() || working()) return
    setWorking(true)
    setError("")
    setLinked("")
    try {
      setLinked(await linkRemoteDevice(code()))
      setCode("")
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setWorking(false)
    }
  }

  return (
    <SettingsGroup title={t("drift.remote.link.title")}>
      <form onSubmit={(event) => void submit(event)}>
        <SettingsRow title={t("drift.remote.link.title")} description={t("drift.remote.link.description")}>
          <div class="flex items-center gap-2">
            <input
              class={`${field} font-mono tracking-widest uppercase sm:w-36`}
              aria-label={t("drift.remote.link.title")}
              placeholder="ABCD-EFGH"
              autocomplete="off"
              spellcheck={false}
              maxLength={12}
              value={code()}
              onInput={(event) => setCode(event.currentTarget.value)}
            />
            <button type="submit" class={button} disabled={!complete() || working()}>{t("drift.remote.link.action")}</button>
          </div>
        </SettingsRow>
      </form>
      <Show when={pending().length}>
        <div class="px-1 py-2 text-xs text-warn" role="status">
          {t("drift.remote.link.waiting", { devices: pending().map((link) => `${link.name} (${link.address})`).join(", ") })}
        </div>
      </Show>
      <Show when={linked()}><div class="px-1 py-2 text-xs text-ok" role="status">{t("drift.remote.link.linked", { name: linked() })}</div></Show>
      <Show when={error()}><div class="px-1 py-2 text-xs text-danger" role="alert">{error()}</div></Show>
    </SettingsGroup>
  )
}

function LinkedDevices() {
  const devices = () => remoteAccessStatus()?.devices ?? []
  const method = (device: RemoteDevice) =>
    device.method === "password" ? t("drift.remote.devices.password") : t("drift.remote.devices.link")
  return (
    <SettingsGroup title={t("drift.remote.devices.title")}>
      <For each={devices()} fallback={<div class="px-1 py-3 text-xs text-ink-faint">{t("drift.remote.devices.empty")}</div>}>
        {(device) => (
          <SettingsRow
            title={device.name}
            description={`${method(device)} · ${t("drift.remote.devices.lastSeen", { time: new Date(device.lastSeenAt).toLocaleString() })}`}
          >
            <button class={button} disabled={remoteAccessBusy()} onClick={() => void revokeRemoteDevice(device.id)}>
              {t("drift.remote.devices.revoke")}
            </button>
          </SettingsRow>
        )}
      </For>
      <Show when={devices().length > 1}>
        <div class="flex justify-end px-1 py-2">
          <button class={button} disabled={remoteAccessBusy()} onClick={() => void revokeRemoteDevice()}>
            {t("drift.remote.devices.revokeAll")}
          </button>
        </div>
      </Show>
    </SettingsGroup>
  )
}

function PasswordSignIn() {
  const username = () => remoteAccessStatus()?.passwordUsername
  const [editing, setEditing] = createSignal(false)
  const [form, setForm] = createSignal({ username: "", password: "", confirm: "" })
  const [error, setError] = createSignal("")
  const update = (key: "username" | "password" | "confirm", value: string) => setForm((current) => ({ ...current, [key]: value }))

  function open() {
    setForm({ username: username() ?? "", password: "", confirm: "" })
    setError("")
    setEditing(true)
  }

  async function save(event: Event) {
    event.preventDefault()
    const value = form()
    if (value.password !== value.confirm) return setError(t("drift.remote.password.mismatch"))
    await setRemotePassword({ username: value.username, password: value.password })
    if (!remoteAccessError()) setEditing(false)
  }

  return (
    <SettingsGroup title={t("drift.remote.password.title")}>
      <SettingsRow
        title={username() ? t("drift.remote.password.on", { username: username()! }) : t("drift.remote.password.off")}
        description={t("drift.remote.password.description")}
      >
        <div class="flex gap-2">
          <button class={button} disabled={remoteAccessBusy()} onClick={open}>
            {username() ? t("drift.remote.password.change") : t("drift.remote.password.setUp")}
          </button>
          <Show when={username()}>
            <button class={button} disabled={remoteAccessBusy()} onClick={() => void setRemotePassword(null)}>
              {t("drift.remote.password.turnOff")}
            </button>
          </Show>
        </div>
      </SettingsRow>
      <Show when={editing()}>
        <form class="space-y-2 px-1 py-3" onSubmit={(event) => void save(event)}>
          <For each={[["username", "text", "username"], ["password", "password", "new-password"], ["confirm", "password", "new-password"]] as const}>
            {([key, type, autocomplete]) => (
              <label class="flex flex-col gap-1 text-xs text-ink-muted sm:flex-row sm:items-center sm:justify-between">
                {t(`drift.remote.password.${key}`)}
                <input
                  class={field}
                  type={type}
                  autocomplete={autocomplete}
                  required
                  value={form()[key]}
                  onInput={(event) => update(key, event.currentTarget.value)}
                />
              </label>
            )}
          </For>
          <p class="text-[0.72rem] text-ink-faint">{t("drift.remote.password.note")}</p>
          <Show when={error()}><p class="text-xs text-danger" role="alert">{error()}</p></Show>
          <div class="flex justify-end gap-2">
            <button type="button" class={button} onClick={() => setEditing(false)}>{t("drift.remote.password.cancel")}</button>
            <button type="submit" class={button} disabled={remoteAccessBusy()}>{t("drift.remote.password.save")}</button>
          </div>
        </form>
      </Show>
    </SettingsGroup>
  )
}

function Encryption() {
  return (
    <SettingsGroup title={t("drift.remote.encryption.title")}>
      <SettingsRow title={t("drift.remote.encryption.https")} description={t("drift.remote.encryption.description")}>
        <span class="size-2 rounded-full bg-ok" />
      </SettingsRow>
      <div class="px-1 py-2.5">
        <div class="text-[0.72rem] text-ink-faint">{t("drift.remote.encryption.fingerprint")}</div>
        <div class="mt-1 font-mono text-[0.68rem] break-all text-ink-muted select-all">{remoteAccessStatus()?.certificateFingerprint}</div>
      </div>
    </SettingsGroup>
  )
}
