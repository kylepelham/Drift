import { createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js"
import { SettingsGroup, SettingsRow } from "./settings-controls"
import { isRemoteRuntime } from "../runtime"
import { Toggle } from "./controls"
import { t } from "../state/i18n"
import {
  lastSeenLabel,
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

const button =
  "rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
const field =
  "w-full rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-ink outline-none focus:border-edge-strong sm:w-56"

export function RemoteAccessSection() {
  return (
    <div class="space-y-5">
      <Show when={!isRemoteRuntime()} fallback={<ThisDevice />}>
        <EnableRow />
        <Show when={remoteAccessStatus()?.enabled && remoteAccessStatus()?.listening}>
          <ConnectDevice />
          <Devices />
          <PasswordSignIn />
          <Certificate />
        </Show>
      </Show>
      <Show when={remoteAccessError() || remoteAccessStatus()?.error}>
        <div class="text-xs text-danger" role="alert">
          {remoteAccessError() || remoteAccessStatus()?.error}
        </div>
      </Show>
    </div>
  )
}

function ThisDevice() {
  onMount(() => void loadRemoteSession().catch(() => undefined))
  return (
    <SettingsGroup title={t("drift.remote.device.title")}>
      <SettingsRow
        title={
          remoteSession()
            ? t("drift.remote.device.signedIn", { name: remoteSession()!.name })
            : t("drift.remote.connected")
        }
        description={t("drift.remote.manageOnDesktop")}
      >
        <button class={button} onClick={() => void signOutRemoteDevice()}>
          {t("drift.remote.device.signOut")}
        </button>
      </SettingsRow>
    </SettingsGroup>
  )
}

function EnableRow() {
  const status = remoteAccessStatus
  onMount(() => {
    void refreshRemoteAccess()
    const stop = listenRemoteAccess()
    onCleanup(() => void stop?.then((release) => release()))
  })
  const starting = () => status()?.enabled && !status()?.listening && !status()?.error
  return (
    <SettingsGroup title={t("drift.remote.title")}>
      <SettingsRow title={t("drift.remote.enable")} description={t("drift.remote.enableDescription")}>
        <div class="flex items-center gap-3">
          <Show when={starting() || remoteStatusTone(status()) === "error"}>
            <span class="text-[0.72rem] text-ink-faint">
              {remoteStatusTone(status()) === "error"
                ? t("drift.remote.statusError")
                : t("drift.remote.statusStarting")}
            </span>
          </Show>
          <Toggle
            label={t("drift.remote.enable")}
            checked={!!status()?.enabled}
            disabled={remoteAccessBusy()}
            onChange={() => void setRemoteAccess(nextRemoteAccessEnabled(status()))}
          />
        </div>
      </SettingsRow>
    </SettingsGroup>
  )
}

function Step(props: { number: number; children: JSX.Element }) {
  return (
    <li class="flex gap-2.5">
      <span class="flex size-5 shrink-0 items-center justify-center rounded-full bg-raised text-[0.68rem] font-semibold text-ink-muted">
        {props.number}
      </span>
      <div class="min-w-0 flex-1 space-y-1.5 text-xs leading-relaxed text-ink-muted">{props.children}</div>
    </li>
  )
}

function ConnectDevice() {
  const url = () => remoteAccessStatus()?.urls[0]
  const pending = () => remoteAccessStatus()?.pendingLinks ?? []
  const [copied, setCopied] = createSignal(false)
  const [copyError, setCopyError] = createSignal(false)
  const [code, setCode] = createSignal("")
  const [linked, setLinked] = createSignal("")
  const [error, setError] = createSignal("")
  const [working, setWorking] = createSignal(false)
  const complete = () => normalizeLinkCode(code()).length === 8

  async function copyAddress() {
    const value = url()
    if (!value) return
    setCopyError(false)
    try {
      await navigator.clipboard.writeText(value)
      setCopied(true)
      setTimeout(() => setCopied(false), 1800)
    } catch {
      setCopyError(true)
    }
  }

  async function link(event: Event) {
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
    <SettingsGroup title={t("drift.remote.connect.title")}>
      <div class="flex flex-col gap-4 px-1 py-3 sm:flex-row">
        <Show when={remoteAccessStatus()?.addressQr}>
          {(svg) => (
            <div
              class="size-32 shrink-0 self-center rounded-lg bg-white p-1.5 sm:self-start [&>svg]:size-full"
              role="img"
              aria-label={url()}
              innerHTML={svg()}
            />
          )}
        </Show>
        <ol class="min-w-0 flex-1 space-y-3">
          <Step number={1}>
            <div>{t("drift.remote.connect.open")}</div>
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-mono text-[0.75rem] text-ink select-all">
                {url() ?? t("drift.remote.noLanAddress")}
              </span>
              <Show when={url()}>
                <button
                  class="text-[0.72rem] text-ink-faint underline-offset-2 hover:text-ink hover:underline"
                  onClick={() => void copyAddress()}
                >
                  {copyError()
                    ? t("drift.remote.clipboardError")
                    : copied()
                      ? t("drift.remote.copied")
                      : t("drift.remote.copy")}
                </button>
              </Show>
            </div>
          </Step>
          <Step number={2}>{t("drift.remote.connect.warning")}</Step>
          <Step number={3}>
            <div>{t("drift.remote.connect.code")}</div>
            <form class="flex items-center gap-2" onSubmit={(event) => void link(event)}>
              <input
                id="remote-link-code"
                class={`${field} font-mono tracking-widest uppercase sm:w-36`}
                aria-label={t("drift.remote.connect.code")}
                placeholder="ABCD-EFGH"
                autocomplete="off"
                spellcheck={false}
                maxLength={12}
                value={code()}
                ref={(element) => pending().length && queueMicrotask(() => element.focus())}
                onInput={(event) => setCode(event.currentTarget.value)}
              />
              <button type="submit" class={button} disabled={!complete() || working()}>
                {t("drift.remote.link.action")}
              </button>
            </form>
            <For each={pending()}>
              {(device) => (
                <div class="flex items-center gap-1.5 text-warn" role="status">
                  <span class="pulse-soft size-1.5 rounded-full bg-warn" />
                  {t("drift.remote.link.waiting", { name: device.name, address: device.address })}
                </div>
              )}
            </For>
            <Show when={linked()}>
              <div class="text-ok" role="status">
                {t("drift.remote.link.linked", { name: linked() })}
              </div>
            </Show>
            <Show when={error()}>
              <div class="text-danger" role="alert">
                {error()}
              </div>
            </Show>
          </Step>
        </ol>
      </div>
    </SettingsGroup>
  )
}

function Devices() {
  const devices = () => remoteAccessStatus()?.devices ?? []
  const describe = (device: RemoteDevice) => {
    const method = device.method === "password" ? t("drift.remote.devices.password") : t("drift.remote.devices.link")
    return `${method} · ${t("drift.remote.devices.lastSeen", { time: lastSeenLabel(device.lastSeenAt) })}`
  }
  return (
    <SettingsGroup title={t("drift.remote.devices.title")}>
      <For
        each={devices()}
        fallback={<div class="px-1 py-3 text-xs text-ink-faint">{t("drift.remote.devices.empty")}</div>}
      >
        {(device) => (
          <SettingsRow title={device.name} description={describe(device)}>
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
  const update = (key: "username" | "password" | "confirm", value: string) =>
    setForm((current) => ({ ...current, [key]: value }))

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
          <For
            each={
              [
                ["username", "text", "username"],
                ["password", "password", "new-password"],
                ["confirm", "password", "new-password"],
              ] as const
            }
          >
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
          <Show when={error()}>
            <p class="text-xs text-danger" role="alert">
              {error()}
            </p>
          </Show>
          <div class="flex justify-end gap-2">
            <button type="button" class={button} onClick={() => setEditing(false)}>
              {t("drift.remote.password.cancel")}
            </button>
            <button type="submit" class={button} disabled={remoteAccessBusy()}>
              {t("drift.remote.password.save")}
            </button>
          </div>
        </form>
      </Show>
    </SettingsGroup>
  )
}

function Certificate() {
  return (
    <div class="rounded-lg border border-edge/80 px-3 py-2 text-xs">
      <div class="text-ink-muted">{t("drift.remote.certificate.title")}</div>
      <div class="mt-2 space-y-2 leading-relaxed text-ink-faint">
        <p>{t("drift.remote.certificate.description")}</p>
        <div>
          <div class="text-[0.72rem]">{t("drift.remote.encryption.fingerprint")}</div>
          <div class="mt-1 font-mono text-[0.68rem] break-all text-ink-muted select-all">
            {remoteAccessStatus()?.certificateFingerprint}
          </div>
        </div>
      </div>
    </div>
  )
}
