import { applyProviderCatalog } from "../state/provider-cache";
import { errorMessage } from "./actions-context";

import type { ProviderAuthMethod } from "./provider-auth";
import type { ActionContext } from "./actions-context";
import type { ProviderAuthResult } from "./actions";

/** Sign-in methods per provider, in the order the settings page lists them. */
const authMethods: Record<
    string,
    { type: "oauth" | "api"; label: string; mode?: "max" | "console" | "chatgpt" | "supergrok" }[]
> = {
    anthropic: [
        { type: "oauth", label: "Claude Pro/Max", mode: "max" },
        { type: "oauth", label: "Anthropic Console", mode: "console" },
        { type: "api", label: "API key" },
    ],
    openai: [
        { type: "oauth", label: "ChatGPT (Plus, Pro, Team)", mode: "chatgpt" },
        { type: "api", label: "API key" },
    ],
    xai: [
        { type: "oauth", label: "SuperGrok", mode: "supergrok" },
        { type: "api", label: "API key" },
    ],
};

/** Provider catalog refresh, API keys and OAuth sign-in. */
export function createProviderActions({ requireClient, state, set, notice }: ActionContext) {
    async function refreshProviders() {
        const providers = await requireClient()
            .providers()
            .catch(() => undefined);
        if (!providers) return false;
        applyProviderCatalog(set, {
            all: providers,
            connected: providers.filter((p) => p.connected).map((p) => p.id),
            default: {},
        });
        return true;
    }

    async function setProviderKey(id: string, key: string): Promise<ProviderAuthResult> {
        await requireClient().setProviderKey(id, key);
        await refreshProviders();
        return { ok: true, connected: state.connected.includes(id) };
    }

    async function disconnectProvider(id: string): Promise<ProviderAuthResult> {
        await requireClient().removeProviderCredentials(id);
        await refreshProviders();
        return { ok: true, connected: state.connected.includes(id) };
    }

    async function providerAuthMethods(): Promise<Record<string, ProviderAuthMethod[]>> {
        return Object.fromEntries(
            Object.entries(authMethods).map(([id, methods]) => [
                id,
                methods.map(({ type, label }) => ({ type, label })),
            ]),
        );
    }

    // The state from startOAuth, needed by the callback for flows the engine completes itself.
    const oauthStates = new Map<string, string>();

    async function providerAuthorize(id: string, method: number) {
        const mode = authMethods[id]?.[method]?.mode;
        if (!mode) throw new Error("this method has no sign-in flow");
        const started = await requireClient().startOAuth(id, mode);
        oauthStates.set(id, started.state);
        // No instructions: settings words each step in the user's language and shows any device code itself.
        return {
            url: started.url,
            method: (started.method === "auto" ? "auto" : "code") as "code" | "auto",
            instructions: "",
            code: started.userCode ?? undefined,
        };
    }

    async function providerCallback(id: string, _method: number, code?: string): Promise<ProviderAuthResult> {
        const oauthState = oauthStates.get(id);
        if (!code && !oauthState) return { ok: false, connected: false };
        try {
            await requireClient().finishOAuth(id, code ?? "", oauthState);
        } catch (cause) {
            notice({
                id: `oauth-${id}`,
                title: "Sign-in failed",
                message: errorMessage(cause),
                variant: "error",
                duration: 10_000,
            });
            return { ok: false, connected: false };
        } finally {
            oauthStates.delete(id);
        }
        await refreshProviders();
        return { ok: true, connected: state.connected.includes(id) };
    }

    return {
        refreshProviders,
        reloadProviders: refreshProviders,
        setProviderKey,
        disconnectProvider,
        providerAuthMethods,
        providerAuthorize,
        providerCallback,
    };
}
