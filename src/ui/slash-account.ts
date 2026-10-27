import { resolveModel, savedChoice } from "../engine/store";
import { prefsFor } from "../state/prefs";
import { t } from "../state/i18n";

import type { SlashPreset } from "./slash";
import type { Engine } from "../engine";

const auto = "auto";

/** The chat's model's provider and its accounts, with the name each goes by. */
function accountsFor(engine: Engine, session: string) {
    const provider = resolveModel(
        engine.state,
        prefsFor(session, savedChoice(engine.state, session)).model,
    )?.providerID;
    const accounts = (engine.state.providerAccounts[provider ?? ""] ?? []).map((account, index) => ({
        ...account,
        name: account.label ?? t("drift.provider.accounts.unnamed", { number: index + 1 }),
    }));

    return { provider, accounts };
}

/** `/account` is offered only where there is more than one account to choose from. */
export function choosesAccount(engine: Engine, session: string | null) {
    return !!session && accountsFor(engine, session).accounts.length > 1;
}

export function accountPresets(engine: Engine, session: string | null): SlashPreset[] {
    if (!session) return [];
    const pinned = engine.state.sessions[session]?.account;
    const { accounts } = accountsFor(engine, session);
    const inUse = (on: boolean) => (on ? `${t("drift.slash.account.current")} ` : "");

    return [
        {
            value: auto,
            label: t("drift.slash.account.auto"),
            description: inUse(!accounts.some((account) => account.id === pinned)) + t("drift.slash.account.autoHint"),
            execute: true,
            literal: true,
        },
        ...accounts.map((account) => ({
            // A label two accounts share would pick the first of them, so those go by id.
            value: accounts.filter((other) => other.name === account.name).length > 1 ? account.id : account.name,
            label: account.name,
            description:
                inUse(account.id === pinned) +
                (account.cyberVerified ? `${t("drift.provider.accounts.cyberVerified")}. ` : "") +
                t("drift.slash.account.pinnedHint"),
            execute: true,
            literal: true,
        })),
    ];
}

/** Sets the chat to the account named by its label, number or id; `auto` hands it back to the account order. */
export async function setSessionAccount(engine: Engine, session: string, args: string) {
    const wanted = args.trim().toLowerCase();
    const { provider, accounts } = accountsFor(engine, session);
    if (!wanted || wanted === auto) {
        await engine.actions.setSessionAccount(session, null);
        engine.actions.notice({ message: t("drift.slash.account.cleared"), variant: "info" });
        return;
    }

    const account = accounts.find(
        (account, index) =>
            account.name.toLowerCase() === wanted || account.id.toLowerCase() === wanted || `${index + 1}` === wanted,
    );
    if (!account) {
        engine.actions.notice({
            message: t("drift.slash.account.unknown", {
                name: args.trim(),
                provider: engine.state.providers.find((entry) => entry.id === provider)?.name ?? provider ?? "",
            }),
            variant: "warning",
        });
        return;
    }

    await engine.actions.setSessionAccount(session, account.id);
    engine.actions.notice({ message: t("drift.slash.account.set", { account: account.name }), variant: "info" });
}
