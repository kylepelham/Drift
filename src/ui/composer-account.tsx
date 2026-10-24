import { Picker, type PickerItem } from "./picker";
import { createMemo, Show } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

const auto = "auto";

/** Which of the provider's accounts this chat uses; shown only when there is more than one to choose from. */
export function ComposerAccount(props: { session: string | null; provider?: string }) {
    const engine = useEngine();
    const accounts = () => engine.state.providerAccounts[props.provider ?? ""] ?? [];
    const items = createMemo<PickerItem[]>(() => [
        { id: auto, label: t("drift.composer.account.auto"), hint: t("drift.composer.account.autoHint") },
        ...accounts().map((account, index) => ({
            id: account.id,
            label: account.label ?? t("drift.provider.accounts.unnamed", { number: index + 1 }),
            hint: t("drift.composer.account.pinnedHint"),
            detail: account.cyberVerified ? t("drift.provider.accounts.cyberVerified") : undefined,
        })),
    ]);
    const selected = () => {
        const pinned = engine.state.sessions[props.session ?? ""]?.account;
        return accounts().find((account) => account.id === pinned)?.id ?? auto;
    };

    return (
        <Show when={props.session && accounts().length > 1}>
            <Picker
                label={t("drift.composer.account")}
                items={items()}
                selected={selected()}
                onPick={(id) => {
                    if (props.session) void engine.actions.setSessionAccount(props.session, id === auto ? null : id);
                }}
            />
        </Show>
    );
}
