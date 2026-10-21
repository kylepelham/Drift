import { createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { isAppRuntime, isRemoteRuntime } from "../runtime";
import { Toggle } from "./controls";
import DOMPurify from "dompurify";
import { t } from "../state/i18n";
import {
    lastSeenLabel,
    listenRemoteAccess,
    loadRemoteSession,
    nextRemoteAccessEnabled,
    pairedDevice,
    pairingReachesAnywhere,
    refreshRemoteAccess,
    remoteAccessBusy,
    remoteAccessError,
    remoteAccessStatus,
    remoteSession,
    remoteStatusTone,
    revokeRemoteDevice,
    setRemoteAccess,
    setRemoteBrowser,
    setRemotePassword,
    setRemoteTunnel,
    showRemotePairing,
    signOutRemoteDevice,
    tunnelUrl,
    type RemoteDevice,
    type RemotePairing,
    type TunnelPhase,
} from "../state/remote-access";

const button =
    "h-8 shrink-0 rounded-md border border-edge-strong bg-raised/50 px-3 text-xs text-ink-muted transition-colors hover:bg-raised hover:text-ink disabled:opacity-40";
const primary =
    "h-8 shrink-0 rounded-md bg-accent px-3 text-xs font-medium text-accent-ink transition-[filter] hover:brightness-110 disabled:opacity-40";
const field =
    "h-8 rounded-md border border-edge-strong bg-bg/70 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent";
const small = "text-[length:calc(0.72rem*var(--settings-scale,1))]";

export function RemoteAccessSection() {
    return (
        <div class="space-y-6">
            <Show when={!isRemoteRuntime()} fallback={<ThisDevice />}>
                <EnableGroup />
                <Show when={remoteAccessStatus()?.enabled && remoteAccessStatus()?.listening}>
                    <PhonePairing />
                    <Connection />
                    <Devices />
                </Show>
            </Show>
            <Show when={remoteAccessError() || remoteAccessStatus()?.error}>
                <div class="text-xs text-danger" role="alert">
                    {remoteAccessError() || remoteAccessStatus()?.error}
                </div>
            </Show>
        </div>
    );
}

function ThisDevice() {
    onMount(() => void loadRemoteSession().catch(() => undefined));
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
                <div class="flex gap-2">
                    <Show when={isAppRuntime()}>
                        <a class={`${button} inline-flex items-center`} href="/app/computers">
                            {t("drift.remote.device.switch")}
                        </a>
                    </Show>
                    <button class={button} onClick={() => void signOutRemoteDevice()}>
                        {t("drift.remote.device.signOut")}
                    </button>
                </div>
            </SettingsRow>
        </SettingsGroup>
    );
}

function EnableGroup() {
    const status = remoteAccessStatus;
    onMount(() => {
        void refreshRemoteAccess();
        const stop = listenRemoteAccess();
        onCleanup(() => void stop?.then((release) => release()));
    });
    const starting = () => status()?.enabled && !status()?.listening && !status()?.error;
    const toggle = () => !remoteAccessBusy() && void setRemoteAccess(nextRemoteAccessEnabled(status()));

    return (
        <SettingsGroup title={t("drift.remote.title")}>
            <SettingsRow
                title={t("drift.remote.enable")}
                description={t("drift.remote.enableDescription")}
                onClick={toggle}
            >
                <div class="flex items-center gap-3">
                    <Show when={starting() || remoteStatusTone(status()) === "error"}>
                        <span class={`${small} text-ink-faint`}>
                            {remoteStatusTone(status()) === "error"
                                ? t("drift.remote.statusError")
                                : t("drift.remote.statusStarting")}
                        </span>
                    </Show>
                    <Toggle
                        label={t("drift.remote.enable")}
                        checked={!!status()?.enabled}
                        disabled={remoteAccessBusy()}
                        onChange={toggle}
                    />
                </div>
            </SettingsRow>
        </SettingsGroup>
    );
}

function Dot(props: { tone: "ok" | "warn" | "danger" | "idle"; pulse?: boolean }) {
    return (
        <span
            class="size-1.5 shrink-0 rounded-full"
            classList={{
                "bg-ok": props.tone === "ok",
                "bg-warn": props.tone === "warn",
                "bg-danger": props.tone === "danger",
                "bg-ink-faint": props.tone === "idle",
                "pulse-soft": !!props.pulse,
            }}
        />
    );
}

/** A line under a switch that belongs to it: what it is doing, or what it offers. */
function Detail(props: { label: JSX.Element; children?: JSX.Element }) {
    return (
        <div class="flex min-h-11 flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-edge/70 py-2 pr-1 pl-5 last:border-b-0">
            <div class={`flex min-w-0 flex-1 items-center gap-2 ${small} text-ink-muted`}>{props.label}</div>
            <div class="flex min-w-0 items-center gap-2">{props.children}</div>
        </div>
    );
}

/** The QR code the Drift app scans: it trusts this PC's certificate and links in one step. */
function PhonePairing() {
    const [pairing, setPairing] = createSignal<{ code: RemotePairing; since: number } | null>(null);
    const [error, setError] = createSignal("");
    const [now, setNow] = createSignal(Date.now());
    const tick = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(tick));

    const linked = () => {
        const shown = pairing();
        return shown ? pairedDevice(remoteAccessStatus(), shown.since) : undefined;
    };
    const expired = () => {
        const shown = pairing();
        return !!shown && shown.code.expiresAt <= now();
    };
    const minutes = () => Math.max(1, Math.ceil(((pairing()?.code.expiresAt ?? 0) - now()) / 60_000));

    async function show() {
        setError("");
        try {
            const since = Date.now();
            setPairing({ code: await showRemotePairing(), since });
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    return (
        <SettingsGroup title={t("drift.remote.app.title")}>
            <SettingsRow title={t("drift.remote.app.link")} description={t("drift.remote.app.description")}>
                <button class={pairing() ? button : primary} onClick={() => void show()}>
                    {pairing() ? t("drift.remote.app.again") : t("drift.remote.app.show")}
                </button>
            </SettingsRow>
            <Show when={pairing()}>
                {(shown) => (
                    <div class="flex flex-col items-center gap-5 px-1 py-4 sm:flex-row sm:items-center">
                        <Show when={!linked() && !expired()}>
                            <div
                                class="size-44 shrink-0 rounded-xl bg-white p-2 shadow-sm [&>svg]:size-full"
                                role="img"
                                aria-label={t("drift.remote.app.qr")}
                                // eslint-disable-next-line solid/no-innerhtml -- DOMPurify sanitises the host-generated QR SVG.
                                innerHTML={DOMPurify.sanitize(shown().code.qr)}
                            />
                        </Show>
                        <div class="min-w-0 flex-1 space-y-2 text-xs">
                            <PairingState
                                linked={linked()}
                                expired={expired()}
                                minutes={minutes()}
                                anywhere={pairingReachesAnywhere(shown().code)}
                            />
                            <button class={button} onClick={() => setPairing(null)}>
                                {linked() ? t("drift.remote.app.done") : t("drift.remote.app.hide")}
                            </button>
                        </div>
                    </div>
                )}
            </Show>
            <Show when={error()}>
                <p class="px-1 pb-3 text-xs text-danger" role="alert">
                    {error()}
                </p>
            </Show>
        </SettingsGroup>
    );
}

function PairingState(props: { linked?: RemoteDevice; expired: boolean; minutes: number; anywhere: boolean }) {
    return (
        <Show
            when={props.linked}
            fallback={
                <Show when={!props.expired} fallback={<p class="text-ink-muted">{t("drift.remote.app.expired")}</p>}>
                    <p class="flex items-center gap-2 text-ink" role="status">
                        <Dot tone="warn" pulse />
                        {t("drift.remote.app.waiting")}
                    </p>
                    <p class="text-ink-faint">{t("drift.remote.app.expires", { minutes: props.minutes })}</p>
                    <p class="text-ink-faint">
                        {props.anywhere ? t("drift.remote.app.reach.anywhere") : t("drift.remote.app.reach.lan")}
                    </p>
                </Show>
            }
        >
            {(device) => (
                <p class="flex items-center gap-2 text-ink" role="status">
                    <Dot tone="ok" />
                    {t("drift.remote.app.linked", { name: device().name })}
                </p>
            )}
        </Show>
    );
}

function Connection() {
    const status = remoteAccessStatus;
    const toggleTunnel = () => !remoteAccessBusy() && void setRemoteTunnel(!status()?.tunnelEnabled);
    const toggleBrowser = () => !remoteAccessBusy() && void setRemoteBrowser(!status()?.browser);

    return (
        <SettingsGroup title={t("drift.remote.connection.title")}>
            <SettingsRow
                title={t("drift.remote.tunnel.title")}
                description={t("drift.remote.tunnel.description")}
                onClick={toggleTunnel}
            >
                <Toggle
                    label={t("drift.remote.tunnel.title")}
                    checked={!!status()?.tunnelEnabled}
                    disabled={remoteAccessBusy()}
                    onChange={toggleTunnel}
                />
            </SettingsRow>
            <Show when={status()?.tunnelEnabled}>
                <TunnelState />
            </Show>
            <SettingsRow
                title={t("drift.remote.browser.title")}
                description={t("drift.remote.browser.description")}
                onClick={toggleBrowser}
            >
                <Toggle
                    label={t("drift.remote.browser.title")}
                    checked={!!status()?.browser}
                    disabled={remoteAccessBusy()}
                    onChange={toggleBrowser}
                />
            </SettingsRow>
            <Show when={status()?.browser}>
                <BrowserAccess />
            </Show>
        </SettingsGroup>
    );
}

const phaseTone: Record<TunnelPhase, "ok" | "warn" | "danger" | "idle"> = {
    off: "idle",
    "setting-up": "warn",
    connecting: "warn",
    online: "ok",
    reconnecting: "warn",
    failed: "danger",
};

function TunnelState() {
    const tunnel = () => remoteAccessStatus()?.tunnel;
    const phase = () => tunnel()?.phase ?? "off";

    return (
        <Detail
            label={
                <>
                    <Dot tone={phaseTone[phase()]} pulse={phaseTone[phase()] === "warn"} />
                    <span class="shrink-0">{t(`drift.remote.tunnel.phase.${phase()}`)}</span>
                    <Show when={tunnel()?.error}>
                        <span class="min-w-0 truncate text-ink-faint" title={tunnel()?.error}>
                            {tunnel()?.error}
                        </span>
                    </Show>
                </>
            }
        >
            <Show
                when={phase() !== "failed"}
                fallback={
                    <button class={button} disabled={remoteAccessBusy()} onClick={() => void setRemoteTunnel(true)}>
                        {t("drift.remote.tunnel.retry")}
                    </button>
                }
            >
                <Show when={tunnelUrl(remoteAccessStatus())}>{(url) => <Address url={url()} />}</Show>
            </Show>
        </Detail>
    );
}

function BrowserAccess() {
    const status = remoteAccessStatus;
    return (
        <>
            <Detail label={t("drift.remote.browser.lan")}>
                <Show
                    when={status()?.lanUrl}
                    fallback={<span class={`${small} text-ink-faint`}>{t("drift.remote.noLanAddress")}</span>}
                >
                    {(url) => <Address url={url()} />}
                </Show>
            </Detail>
            <Show when={tunnelUrl(status())}>
                {(url) => (
                    <Detail label={t("drift.remote.browser.anywhere")}>
                        <Address url={url()} />
                    </Detail>
                )}
            </Show>
            <Password />
            <Detail label={t("drift.remote.certificate.title")}>
                <span
                    class="max-w-80 font-mono text-[length:calc(0.66rem*var(--settings-scale,1))] break-all text-ink-faint select-all"
                    title={t("drift.remote.certificate.description")}
                >
                    {status()?.certificateFingerprint}
                </span>
            </Detail>
        </>
    );
}

/** An address to open on the other device, with a button that copies it. */
function Address(props: { url: string }) {
    const [copied, setCopied] = createSignal<"" | "copied" | "failed">("");
    async function copy() {
        try {
            await navigator.clipboard.writeText(props.url);
            setCopied("copied");
        } catch {
            setCopied("failed");
        }
        setTimeout(() => setCopied(""), 1800);
    }

    return (
        <>
            <span class={`min-w-0 truncate font-mono ${small} text-ink select-all`} title={props.url}>
                {props.url}
            </span>
            <button class={button} onClick={() => void copy()}>
                {t(copyLabel[copied()])}
            </button>
        </>
    );
}

const copyLabel = {
    "": "drift.remote.copy",
    copied: "drift.remote.copied",
    failed: "drift.remote.clipboardError",
} as const;

function Password() {
    const username = () => remoteAccessStatus()?.passwordUsername;
    const [editing, setEditing] = createSignal(false);
    const [form, setForm] = createSignal({ username: "", password: "", confirm: "" });
    const [error, setError] = createSignal("");
    const update = (key: "username" | "password" | "confirm", value: string) =>
        setForm((current) => ({ ...current, [key]: value }));

    function open() {
        setForm({ username: username() ?? "", password: "", confirm: "" });
        setError("");
        setEditing(true);
    }

    async function save(event: Event) {
        event.preventDefault();
        const value = form();
        if (value.password !== value.confirm) return setError(t("drift.remote.password.mismatch"));
        await setRemotePassword({ username: value.username, password: value.password });
        if (!remoteAccessError()) setEditing(false);
    }

    return (
        <>
            <Detail
                label={
                    <Show
                        when={username()}
                        fallback={
                            <>
                                <Dot tone="warn" />
                                {t("drift.remote.password.unset")}
                            </>
                        }
                    >
                        {(name) => t("drift.remote.password.signedInAs", { username: name() })}
                    </Show>
                }
            >
                <button class={username() ? button : primary} disabled={remoteAccessBusy()} onClick={open}>
                    {username() ? t("drift.remote.password.change") : t("drift.remote.password.setUp")}
                </button>
                <Show when={username()}>
                    <button class={button} disabled={remoteAccessBusy()} onClick={() => void setRemotePassword(null)}>
                        {t("drift.remote.password.turnOff")}
                    </button>
                </Show>
            </Detail>
            <Show when={editing()}>
                <form class="space-y-2 border-b border-edge/70 py-3 pr-1 pl-5" onSubmit={(event) => void save(event)}>
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
                            <label class="flex flex-wrap items-center justify-between gap-2 text-xs text-ink-muted">
                                {t(`drift.remote.password.${key}`)}
                                <input
                                    class={field}
                                    style={{ width: pickerWidth }}
                                    type={type}
                                    autocomplete={autocomplete}
                                    required
                                    value={form()[key]}
                                    onInput={(event) => update(key, event.currentTarget.value)}
                                />
                            </label>
                        )}
                    </For>
                    <p class={`${small} text-ink-faint`}>{t("drift.remote.password.note")}</p>
                    <Show when={error()}>
                        <p class="text-xs text-danger" role="alert">
                            {error()}
                        </p>
                    </Show>
                    <div class="flex justify-end gap-2">
                        <button type="button" class={button} onClick={() => setEditing(false)}>
                            {t("drift.remote.password.cancel")}
                        </button>
                        <button type="submit" class={primary} disabled={remoteAccessBusy()}>
                            {t("drift.remote.password.save")}
                        </button>
                    </div>
                </form>
            </Show>
        </>
    );
}

function Devices() {
    const devices = () => remoteAccessStatus()?.devices ?? [];
    const describe = (device: RemoteDevice) => {
        const method = t(`drift.remote.devices.${device.method}`);
        return `${method} · ${t("drift.remote.devices.lastSeen", { time: lastSeenLabel(device.lastSeenAt) })}`;
    };

    return (
        <SettingsGroup
            title={t("drift.remote.devices.title")}
            action={
                <Show when={devices().length > 1}>
                    <button
                        class={`${small} text-ink-faint transition-colors hover:text-danger disabled:opacity-40`}
                        disabled={remoteAccessBusy()}
                        onClick={() => void revokeRemoteDevice()}
                    >
                        {t("drift.remote.devices.revokeAll")}
                    </button>
                </Show>
            }
        >
            <For
                each={devices()}
                fallback={<div class="px-1 py-3 text-xs text-ink-faint">{t("drift.remote.devices.empty")}</div>}
            >
                {(device) => (
                    <SettingsRow title={device.name} description={describe(device)}>
                        <button
                            class={button}
                            disabled={remoteAccessBusy()}
                            onClick={() => void revokeRemoteDevice(device.id)}
                        >
                            {t("drift.remote.devices.revoke")}
                        </button>
                    </SettingsRow>
                )}
            </For>
        </SettingsGroup>
    );
}
