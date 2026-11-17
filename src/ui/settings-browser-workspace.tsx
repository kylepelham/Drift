import { panelForm, panelLine, panelRemove, SettingsDisclosure, SettingsGroup, SettingsRow } from "./settings-controls";
import { createResource, createSignal, For, Show } from "solid-js";
import { activeWorkspace } from "../state/workspaces";
import { useEngine } from "../engine";
import { IconTrash } from "./icons";
import { t } from "../state/i18n";

import type { BrowserCredential, WorkspaceBrowser } from "../engine/native/client";

const button =
    "rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:bg-raised hover:text-ink disabled:opacity-40";
const field =
    "min-w-0 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-ink outline-none focus:border-edge-strong";

/** The current workspace's drift.json `browser` section, as read, and the sign-ins stored for it. */
export function BrowserWorkspaceGroup() {
    const engine = useEngine();
    const [setup, { refetch }] = createResource(
        () => activeWorkspace()?.path ?? "",
        () => engine.actions.workspaceBrowser(),
    );

    return (
        <Show
            when={activeWorkspace()}
            fallback={
                <SettingsGroup title={t("drift.settings.browser.workspace")}>
                    <p class="py-3 text-xs text-ink-faint">{t("drift.settings.browser.noWorkspace")}</p>
                </SettingsGroup>
            }
        >
            {(workspace) => (
                <SettingsGroup
                    title={t("drift.settings.browser.workspaceNamed", { name: workspace().name })}
                    action={
                        <button class={button} onClick={() => void refetch()}>
                            {t("drift.settings.browser.reload")}
                        </button>
                    }
                >
                    <ProjectRows setup={setup()} />
                    <CredentialsRow credentials={setup()?.credentials ?? []} />
                </SettingsGroup>
            )}
        </Show>
    );
}

/** What the project's drift.json sets, one row each; the agent or the user edits the file itself. */
function ProjectRows(props: { setup?: WorkspaceBrowser }) {
    const project = () => props.setup?.project;
    const set = () => {
        const current = project();
        return !!current && (!!current.engine || !!current.baseUrl || !!current.login || !!current.notes);
    };
    const patterns = () =>
        Object.entries(project()?.engines ?? {})
            .map(([engine, list]) => `${engine}: ${list.join(", ")}`)
            .join("; ");

    return (
        <Show
            when={set()}
            fallback={<p class="py-3 text-xs text-ink-faint">{t("drift.settings.browser.noProject")}</p>}
        >
            <Show when={project()?.engine}>
                {(engine) => (
                    <SettingsRow title={t("drift.settings.browser.project.engine")} description={engine()}>
                        <span />
                    </SettingsRow>
                )}
            </Show>
            <Show when={project()?.baseUrl}>
                {(base) => (
                    <SettingsRow title={t("drift.settings.browser.project.baseUrl")} description={base()}>
                        <span />
                    </SettingsRow>
                )}
            </Show>
            <Show when={patterns()}>
                <SettingsRow title={t("drift.settings.browser.project.patterns")} description={patterns()}>
                    <span />
                </SettingsRow>
            </Show>
            <Show when={project()?.login}>
                {(login) => (
                    <SettingsRow
                        title={t("drift.settings.browser.project.login")}
                        description={t("drift.settings.browser.project.loginWith", { name: login().credential })}
                    >
                        <span />
                    </SettingsRow>
                )}
            </Show>
            <Show when={project()?.notes}>
                {(notes) => (
                    <SettingsRow title={t("drift.settings.browser.project.notes")} description={notes()}>
                        <span />
                    </SettingsRow>
                )}
            </Show>
        </Show>
    );
}

/** Sign-ins for this workspace, listed in a panel the row opens: names and users here, passwords only in the keychain. */
function CredentialsRow(props: { credentials: BrowserCredential[] }) {
    const engine = useEngine();
    // What the engine last answered; a save or removal answers with the whole list.
    const [saved, setSaved] = createSignal<BrowserCredential[]>();
    const list = () => saved() ?? props.credentials;
    const [name, setName] = createSignal("");
    const [user, setUser] = createSignal("");
    const [password, setPassword] = createSignal("");
    const [domain, setDomain] = createSignal("");
    const [error, setError] = createSignal("");
    const ready = () => !!name().trim() && !!user().trim();

    function edit(credential: BrowserCredential) {
        setName(credential.name);
        setUser(credential.user);
        setDomain(credential.domain ?? "");
        setPassword("");
    }

    async function save() {
        setError("");
        const body = { user: user(), domain: domain(), password: password() || null };
        await engine.actions
            .saveBrowserCredential(name().trim(), body)
            .then((stored) => {
                setSaved(stored);
                setName("");
                setUser("");
                setPassword("");
                setDomain("");
            })
            .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)));
    }

    async function remove(credentialName: string) {
        setError("");
        await engine.actions
            .removeBrowserCredential(credentialName)
            .then(setSaved)
            .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)));
    }

    return (
        <SettingsDisclosure
            title={t("drift.settings.browser.credentials")}
            description={credentialsSummary(list())}
            label={t("drift.settings.browser.credentialsManage")}
        >
            <For each={list()}>
                {(credential) => (
                    <div class={panelLine}>
                        <button
                            class="flex min-w-0 flex-1 items-baseline gap-2 text-left"
                            title={t("drift.settings.browser.editCredential", { name: credential.name })}
                            onClick={() => edit(credential)}
                        >
                            <span class="shrink-0 text-xs text-ink">{credential.name}</span>
                            <span class="min-w-0 truncate text-xs text-ink-faint">{credentialDetail(credential)}</span>
                        </button>
                        <button
                            class={panelRemove}
                            aria-label={t("drift.settings.browser.removeCredential", { name: credential.name })}
                            title={t("drift.settings.browser.removeCredential", { name: credential.name })}
                            onClick={() => void remove(credential.name)}
                        >
                            <IconTrash class="size-3.5" />
                        </button>
                    </div>
                )}
            </For>
            <div class={panelForm}>
                <input
                    class={`${field} w-28`}
                    aria-label={t("drift.settings.browser.credentialName")}
                    placeholder={t("drift.settings.browser.credentialName")}
                    value={name()}
                    onInput={(event) => setName(event.currentTarget.value)}
                />
                <input
                    class={`${field} min-w-28 flex-1`}
                    aria-label={t("drift.settings.browser.user")}
                    placeholder={t("drift.settings.browser.user")}
                    autocomplete="off"
                    value={user()}
                    onInput={(event) => setUser(event.currentTarget.value)}
                />
                <input
                    class={`${field} min-w-28 flex-1`}
                    type="password"
                    autocomplete="new-password"
                    aria-label={t("drift.settings.browser.password")}
                    placeholder={t("drift.settings.browser.password")}
                    value={password()}
                    onInput={(event) => setPassword(event.currentTarget.value)}
                />
                <input
                    class={`${field} w-24`}
                    aria-label={t("drift.settings.browser.domain")}
                    placeholder={t("drift.settings.browser.domain")}
                    value={domain()}
                    onInput={(event) => setDomain(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && ready() && void save()}
                />
                <button class={button} disabled={!ready()} onClick={() => void save()}>
                    {t("common.save")}
                </button>
            </div>
            <p class="px-3 pb-2.5 text-xs leading-relaxed text-ink-faint">
                {t("drift.settings.browser.credentialsHint")}
            </p>
            <Show when={error()}>
                <p class="px-3 pb-2.5 text-xs text-danger">{error()}</p>
            </Show>
        </SettingsDisclosure>
    );
}

/** A sign-in's user, domain and whether its password is stored, after its name. */
function credentialDetail(credential: BrowserCredential) {
    const password = credential.hasPassword
        ? "drift.settings.browser.passwordStored"
        : "drift.settings.browser.noPassword";

    return [credential.user, credential.domain, t(password)].filter(Boolean).join(" \u00b7 ");
}

/** The sign-ins as one line, for the row while it is closed. */
function credentialsSummary(credentials: BrowserCredential[]) {
    if (credentials.length === 0) return t("drift.settings.browser.noCredentials");

    return credentials.map((credential) => credential.name).join(" \u00b7 ");
}
