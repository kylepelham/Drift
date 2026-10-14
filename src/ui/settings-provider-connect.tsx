import { ProviderAccounts } from "./settings-provider-accounts";
import { authorizationPrompt } from "../engine/provider-auth";
import { createSignal, For, Show } from "solid-js";
import { openExternal } from "../shell";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { ProviderNotice } from "./settings-providers";
import type { ProviderAuthMethod } from "./settings";

export function ProviderConnect(props: {
    providerId: string;
    providerName: string;
    connected: boolean;
    methods: ProviderAuthMethod[];
    onNotice: (notice: ProviderNotice) => void;
}) {
    const engine = useEngine();
    const [methodIndex, setMethodIndex] = createSignal(0);
    const [key, setKey] = createSignal("");
    const [code, setCode] = createSignal("");
    const [pending, setPending] = createSignal<"connect" | "disconnect" | null>(null);
    const [error, setError] = createSignal("");
    const [authorization, setAuthorization] = createSignal<{
        url: string;
        method: string;
        instructions: string;
        code?: string;
    } | null>(null);
    const method = () => props.methods[methodIndex()] ?? props.methods[0];
    const accounts = () => engine.state.providerAccounts[props.providerId] ?? [];
    const signedIn = () => accounts().filter((account) => account.signedIn);

    function fail(message: string) {
        setError(message);
        setPending(null);
        props.onNotice({ tone: "error", text: message });
    }

    async function finish(request: Promise<{ ok: boolean; connected: boolean }>) {
        const result = await request.catch(() => ({ ok: false, connected: props.connected }));
        if (!result.ok) {
            fail(t("drift.provider.connectFailed", { provider: props.providerName }));
            return;
        }
        if (!result.connected) {
            fail(t("drift.provider.savedUnavailable", { provider: props.providerName }));
            return;
        }

        setKey("");
        setCode("");
        setPending(null);
        props.onNotice({ tone: "success", text: t("drift.provider.connected", { provider: props.providerName }) });
    }

    async function connectApi() {
        if (!key().trim()) return;

        setPending("connect");
        setError("");
        await finish(engine.actions.setProviderKey(props.providerId, key().trim()));
    }

    async function startOauth() {
        setPending("connect");
        setError("");

        const auth = await engine.actions.providerAuthorize(props.providerId, methodIndex()).catch(() => null);
        if (!auth) {
            setError(t("drift.provider.signInStartFailed"));
            setPending(null);
            props.onNotice({
                tone: "error",
                text: t("drift.provider.signInStartFailedFor", { provider: props.providerName }),
            });
            return;
        }

        setAuthorization(auth);
        openExternal(auth.url);
        if (auth.method === "auto") {
            await finish(engine.actions.providerCallback(props.providerId, methodIndex()));
            setAuthorization(null);
            return;
        }

        setPending(null);
    }

    async function submitCode() {
        if (!code().trim()) return;

        setPending("connect");
        setError("");
        await finish(engine.actions.providerCallback(props.providerId, methodIndex(), code().trim()));
    }

    function cancelAuthorization() {
        setAuthorization(null);
        setPending(null);
    }

    async function disconnect() {
        setPending("disconnect");
        setError("");

        const result = await engine.actions.disconnectProvider(props.providerId);
        setPending(null);
        if (!result.ok) {
            fail(t("drift.provider.disconnectFailed", { provider: props.providerName }));
            return;
        }
        if (result.connected) {
            props.onNotice({
                tone: "warning",
                text: t("drift.provider.credentialRemovedStillConnected", { provider: props.providerName }),
            });
            return;
        }

        props.onNotice({ tone: "success", text: t("drift.provider.disconnected", { provider: props.providerName }) });
    }

    return (
        <div class="mx-3 mb-3 space-y-3 rounded-lg border border-edge bg-surface/55 p-3 shadow-sm shadow-black/5">
            <Show when={props.methods.length > 1 || props.connected}>
                <div class="flex items-center gap-2">
                    <Show when={props.methods.length > 1} fallback={<div class="flex-1" />}>
                        <div class="flex min-w-0 flex-1 flex-wrap gap-1.5">
                            <For each={props.methods}>
                                {(item, index) => (
                                    <button
                                        class="rounded-full border px-3 py-1 text-xs transition-colors"
                                        classList={{
                                            "border-accent/50 bg-accent/10 text-ink": index() === methodIndex(),
                                            "border-edge text-ink-faint hover:border-edge-strong hover:text-ink-muted":
                                                index() !== methodIndex(),
                                        }}
                                        onClick={() => {
                                            setMethodIndex(index());
                                            setAuthorization(null);
                                            setError("");
                                        }}
                                    >
                                        {item.label}
                                    </button>
                                )}
                            </For>
                        </div>
                    </Show>
                    <Show when={props.connected}>
                        <button
                            class="h-8 shrink-0 rounded-md border border-danger/40 px-3 text-xs font-medium text-danger transition-colors hover:border-danger/60 hover:bg-danger/10 disabled:opacity-40"
                            disabled={pending() !== null}
                            onClick={() => void disconnect()}
                        >
                            {pending() === "disconnect" ? t("drift.provider.disconnecting") : t("common.disconnect")}
                        </button>
                    </Show>
                </div>
            </Show>
            <Show when={method()?.type === "api"}>
                <div class="flex gap-2">
                    <input
                        type="password"
                        class="h-9 min-w-0 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
                        placeholder={t("provider.connect.apiKey.placeholder")}
                        value={key()}
                        onInput={(event) => setKey(event.currentTarget.value)}
                        onKeyDown={(event) => event.key === "Enter" && void connectApi()}
                    />
                    <button
                        class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
                        disabled={pending() !== null || !key().trim()}
                        onClick={() => void connectApi()}
                    >
                        {t(providerConnectLabel(pending() === "connect", props.connected))}
                    </button>
                </div>
            </Show>
            <Show when={method()?.type === "oauth" && signedIn().length > 0}>
                <ProviderAccounts
                    providerId={props.providerId}
                    providerName={props.providerName}
                    accounts={signedIn()}
                    onNotice={props.onNotice}
                />
            </Show>
            <Show when={method()?.type === "oauth"}>
                <Show
                    when={authorization()}
                    fallback={
                        <button
                            class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
                            disabled={pending() !== null}
                            onClick={() => void startOauth()}
                        >
                            {signInLabel(pending() === "connect", signedIn().length > 0, method()?.label)}
                        </button>
                    }
                >
                    {(auth) => (
                        <div class="space-y-2">
                            <AuthorizationHint auth={auth()} onCancel={cancelAuthorization} />
                            <Show when={auth().method === "code"}>
                                <div class="flex gap-2">
                                    <input
                                        class="h-9 min-w-0 flex-1 rounded-md border border-edge bg-overlay/50 px-2.5 text-sm outline-none transition-colors focus:border-edge-strong"
                                        placeholder={t("provider.connect.oauth.code.placeholder")}
                                        value={code()}
                                        onInput={(event) => setCode(event.currentTarget.value)}
                                        onKeyDown={(event) => event.key === "Enter" && void submitCode()}
                                    />
                                    <button
                                        class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
                                        disabled={pending() !== null || !code().trim()}
                                        onClick={() => void submitCode()}
                                    >
                                        {pending() === "connect"
                                            ? t("provider.connect.status.inProgress")
                                            : t("common.submit")}
                                    </button>
                                </div>
                            </Show>
                            <Show when={auth().method === "auto" && pending() === "connect"}>
                                <div class="pulse-soft text-xs text-ink-faint">
                                    {t("provider.connect.status.waiting")}
                                </div>
                            </Show>
                        </div>
                    )}
                </Show>
            </Show>
            <Show when={error()}>
                <div class="rounded-md border border-danger/30 bg-danger/5 px-2.5 py-2 text-xs text-danger">
                    {error()}
                </div>
            </Show>
        </div>
    );
}

function AuthorizationHint(props: {
    auth: { url: string; method: string; instructions: string; code?: string };
    onCancel: () => void;
}) {
    const prompt = () => (props.auth.code ? { code: props.auth.code } : authorizationPrompt(props.auth.instructions));
    const [copied, setCopied] = createSignal(false);
    const fallback = () =>
        props.auth.method === "code" ? t("drift.provider.pasteCode") : t("drift.provider.finishInBrowser");
    const link = "text-xs text-ink-faint underline-offset-2 transition-colors hover:text-ink hover:underline";

    const copyLink = () => {
        void navigator.clipboard.writeText(props.auth.url).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
        });
    };

    return (
        <div class="space-y-2">
            <Show
                when={prompt().code}
                fallback={<div class="text-xs text-ink-muted">{prompt().text ?? fallback()}</div>}
            >
                {(code) => (
                    <div class="flex items-center gap-3">
                        <span class="text-xs text-ink-muted">{t("drift.provider.enterCode")}</span>
                        <button
                            class="rounded-md border border-edge bg-overlay/60 px-2.5 py-1 font-mono text-sm tracking-widest text-ink select-text"
                            title={t("drift.provider.copyCode")}
                            onClick={() => void navigator.clipboard.writeText(code())}
                        >
                            {code()}
                        </button>
                    </div>
                )}
            </Show>
            <div class="flex items-center gap-3">
                <button class={link} onClick={() => openExternal(props.auth.url)}>
                    {t("drift.provider.openAgain")}
                </button>
                <button class={link} onClick={copyLink}>
                    {copied() ? t("drift.provider.linkCopied") : t("drift.provider.copyLink")}
                </button>
                <button class={link} onClick={() => props.onCancel()}>
                    {t("common.cancel")}
                </button>
            </div>
        </div>
    );
}

function providerConnectLabel(connecting: boolean, connected: boolean) {
    if (connecting) return "provider.connect.status.inProgress";

    return connected ? "common.save" : "common.connect";
}

/** Once a sign-in exists, signing in again adds another account to take turns with it. */
function signInLabel(connecting: boolean, hasAccounts: boolean, method: string | undefined) {
    if (connecting) return t("provider.connect.status.waiting");
    if (hasAccounts) return t("drift.provider.accounts.add");

    return t("drift.provider.signInWith", { method: method ?? t("drift.provider.browser") });
}
