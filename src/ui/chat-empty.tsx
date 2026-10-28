import { activeWorkspace, pickWorkspace, workspaces, workspacesReady } from "../state/workspaces";
import { splashMascotAnimation } from "../state/startup";
import { openSettings } from "../state/settings";
import { needsProvider } from "../engine/store";
import { IconPlus, IconPlug } from "./icons";
import { actionFailed } from "./workspaces";
import { Match, Switch } from "solid-js";
import { useEngine } from "../engine";
import { DriftLogo } from "./logo";
import { t } from "../state/i18n";

import type { JSX } from "solid-js";

const quotes = 12;

/** The chat pane with no thread selected: the logo swimming as on the splash, where to start, and a line picked fresh each time. */
export function EmptyState() {
    const engine = useEngine();
    const quote = `drift.chat.empty.quote${Math.floor(Math.random() * quotes) + 1}`;
    // A new user's two steps, in order: a folder to work in, then a model to talk to.
    const firstRun = () => workspacesReady() && workspaces().length === 0;
    const noModel = () => !!activeWorkspace() && needsProvider(engine.state);
    const add = () =>
        void pickWorkspace().catch((cause: unknown) => actionFailed(engine, t("drift.sidebar.addWorkspace"), cause));

    return (
        <div class="flex flex-1 flex-col items-center justify-center gap-3 select-none">
            <div class="fade-up" data-mascot={splashMascotAnimation()}>
                <DriftLogo class="startup-jelly" label="Drift" />
            </div>{" "}
            <div class="fade-up max-w-md text-center text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
                <Switch>
                    <Match when={firstRun()}>{t("drift.chat.empty.firstWorkspace")}</Match>
                    <Match when={!activeWorkspace()}>{t("drift.chat.empty.noWorkspace")}</Match>
                    <Match when={noModel()}>{t("drift.chat.empty.connectProvider")}</Match>
                    <Match when={activeWorkspace()}>{t("drift.chat.empty.promptHint")}</Match>
                </Switch>
            </div>
            <Switch
                fallback={
                    <div class="fade-up text-xs text-ink-faint" style={{ "animation-delay": "160ms" }}>
                        {t(quote)}
                    </div>
                }
            >
                <Match when={firstRun()}>
                    <StepButton
                        icon={<IconPlus class="size-3.5" />}
                        label={t("drift.sidebar.addWorkspace")}
                        onClick={add}
                    />
                </Match>
                <Match when={noModel()}>
                    <StepButton
                        icon={<IconPlug class="size-3.5" />}
                        label={t("drift.chat.empty.connectModel")}
                        onClick={() => openSettings("Providers")}
                    />
                </Match>
            </Switch>
        </div>
    );
}

function StepButton(props: { icon: JSX.Element; label: string; onClick: () => void }) {
    return (
        <button
            type="button"
            class="fade-up inline-flex items-center gap-1.5 rounded-md border border-edge bg-raised px-3 py-1.5 text-xs text-ink transition-colors hover:border-edge-strong"
            style={{ "animation-delay": "160ms" }}
            onClick={() => props.onClick()}
        >
            {props.icon}
            {props.label}
        </button>
    );
}
