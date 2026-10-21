import { bundleAsks, bundleRunsCode, type RegistryPlugin } from "../state/plugin-registry";
import { IconArrowUp, IconArrowUpRight, IconCheck, IconPlus, IconTrash } from "./icons";
import { createSignal, For, Show, type JSX } from "solid-js";
import { Badge, Chevron, Toggle } from "./controls";
import { mcpStatusLabel } from "./mcp/manager";
import { openExternal } from "../shell";
import { LogoTile } from "./logo-tile";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { Bundle, BundlePartKind } from "../engine/native/client";

const bundlesFolder = "~/.config/drift/bundles";

type Counted = Record<"skills" | "agents" | "commands" | "servers" | "programs", unknown[]>;

/** Each kind of part it holds with how many, on one line. */
function bundleSummary(bundle: Counted) {
    const counts: [number, string][] = [
        [bundle.skills.length, "skills"],
        [bundle.agents.length, "agents"],
        [bundle.commands.length, "commands"],
        [bundle.servers.length, "servers"],
    ];
    const parts = counts
        .filter(([count]) => count > 0)
        .map(([count, key]) => `${t(`drift.bundles.section.${key}`)} ${count}`);
    if (bundle.programs.length) parts.push(t("drift.bundles.section.programs"));

    return parts.join(" · ");
}

/** An installed bundle in the Installed list: its one switch, Remove, and its screen behind it. */
export function BundleRow(props: {
    bundle: Bundle;
    disabled: boolean;
    confirming: boolean;
    onEnabled: (enabled: boolean) => void;
    onOpen: () => void;
    onRemove: () => void;
}) {
    return (
        <div
            class="flex cursor-pointer items-center gap-3 border-b border-edge/70 px-3 py-2.5 last:border-b-0 hover:bg-raised/40"
            role="button"
            tabIndex={0}
            onClick={() => props.onOpen()}
            onKeyDown={(event) => {
                if (event.target !== event.currentTarget || (event.key !== "Enter" && event.key !== " ")) return;
                event.preventDefault();
                props.onOpen();
            }}
        >
            <Chevron open={false} />
            <LogoTile image={props.bundle.image ?? undefined} title={props.bundle.name} />
            <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                    <span class="truncate text-sm font-medium text-ink">{props.bundle.name}</span>
                    <Badge tone="accent">{t("drift.bundles.badge")}</Badge>
                </div>
                <div class="mt-0.5 truncate text-xs text-ink-faint">
                    <span classList={{ "text-ok": props.bundle.enabled }}>
                        {t(props.bundle.enabled ? "drift.plugins.loaded" : "drift.plugins.off")}
                    </span>
                    {" · "}
                    {bundleSummary(props.bundle)}
                </div>
            </div>
            <RemoveButton disabled={props.disabled} confirming={props.confirming} onRemove={props.onRemove} />
            <Toggle
                label={props.bundle.name}
                checked={props.bundle.enabled}
                disabled={props.disabled}
                onChange={() => props.onEnabled(!props.bundle.enabled)}
            />
        </div>
    );
}

function RemoveButton(props: { disabled: boolean; confirming: boolean; onRemove: () => void }) {
    const label = () => t(props.confirming ? "drift.plugins.confirmRemove" : "drift.plugins.remove");

    return (
        <button
            type="button"
            disabled={props.disabled}
            title={label()}
            aria-label={label()}
            class="flex items-center gap-1 rounded-md border border-danger/40 px-2 py-1 text-xs text-danger hover:bg-danger/10 disabled:opacity-40"
            onClick={(event) => {
                event.stopPropagation();
                props.onRemove();
            }}
        >
            {props.confirming ? label() : <IconTrash class="size-3.5" />}
        </button>
    );
}

function BackButton(props: { onBack: () => void }) {
    return (
        <button class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink" onClick={() => props.onBack()}>
            <IconArrowUp class="size-3.5 -rotate-90" />
            {t("drift.mcp.registry.back")}
        </button>
    );
}

function SourceLink(props: { source?: string | null; folder: string }) {
    return (
        <div class="mt-2 flex flex-wrap items-center gap-3 text-xs">
            <Show when={props.source}>
                {(source) => (
                    <button
                        class="flex items-center gap-0.5 text-accent hover:underline"
                        onClick={() => openExternal(source())}
                    >
                        {t("drift.plugins.source")}
                        <IconArrowUpRight class="size-3" />
                    </button>
                )}
            </Show>
            <span class="truncate font-mono text-ink-faint">{props.folder}</span>
        </div>
    );
}

function SectionTitle(props: { children: JSX.Element }) {
    return (
        <div class="pt-4 pb-1.5 text-[length:calc(0.68rem*var(--settings-scale,1))] font-semibold tracking-wide text-ink-faint uppercase">
            {props.children}
        </div>
    );
}

function PartRow(props: {
    name: string;
    description?: string;
    detail?: JSX.Element;
    dimmed: boolean;
    children?: JSX.Element;
}) {
    return (
        <div
            class="flex min-h-11 items-center gap-3 border-b border-edge/70 px-3 py-2 last:border-b-0"
            classList={{ "opacity-60": props.dimmed }}
        >
            <div class="min-w-0 flex-1">
                <div class="flex min-w-0 items-center gap-2">
                    <span class="truncate text-[length:calc(0.82rem*var(--settings-scale,1))] font-medium text-ink">
                        {props.name}
                    </span>
                    {props.detail}
                </div>
                <Show when={props.description}>
                    <div class="truncate text-xs text-ink-faint" title={props.description}>
                        {props.description}
                    </div>
                </Show>
            </div>
            {props.children}
        </div>
    );
}

type PartChange = (kind: BundlePartKind, name: string, enabled: boolean) => void;

/** The switches for one kind of part: skills, agents or commands. */
function PartSection(props: {
    title: string;
    kind: BundlePartKind;
    parts: Bundle["skills"];
    bundleOn: boolean;
    disabled: boolean;
    onPart: PartChange;
}) {
    return (
        <Show when={props.parts.length}>
            <SectionTitle>{props.title}</SectionTitle>
            <div class="border-y border-edge/80">
                <For each={props.parts}>
                    {(part) => (
                        <PartRow
                            name={part.name}
                            description={part.description}
                            dimmed={!part.enabled || !props.bundleOn}
                        >
                            <Toggle
                                label={part.name}
                                checked={part.enabled}
                                disabled={props.disabled}
                                onChange={() => props.onPart(props.kind, part.name, !part.enabled)}
                            />
                        </PartRow>
                    )}
                </For>
            </div>
        </Show>
    );
}

/** A bundle's MCP servers, each with its status as the MCP list carries it and a sign-in when one is asked for. */
function ServerSection(props: { bundle: Bundle; disabled: boolean; onPart: PartChange }) {
    const engine = useEngine();
    const [failure, setFailure] = createSignal("");
    const signIn = async (server: string) => {
        setFailure("");
        try {
            openExternal(await engine.actions.mcpSignIn(server));
        } catch (error) {
            setFailure(error instanceof Error ? error.message : String(error));
        }
    };

    return (
        <Show when={props.bundle.servers.length}>
            <SectionTitle>{t("drift.bundles.section.servers")}</SectionTitle>
            <Show when={failure()}>
                <div
                    role="alert"
                    class="mb-2 rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-xs text-danger"
                >
                    {failure()}
                </div>
            </Show>
            <div class="border-y border-edge/80">
                <For each={props.bundle.servers}>
                    {(part) => {
                        const status = () => engine.state.mcpServers[part.server];
                        const label = () => {
                            const server = status();
                            return server ? mcpStatusLabel(server, false) : undefined;
                        };
                        return (
                            <PartRow
                                name={part.name}
                                dimmed={!part.enabled || !props.bundle.enabled}
                                detail={
                                    <Show when={label()}>
                                        {(shown) => (
                                            <span class={`truncate text-xs ${shown().tone}`}>{shown().text}</span>
                                        )}
                                    </Show>
                                }
                            >
                                <Show when={status()?.needsSignIn}>
                                    <button
                                        class="rounded-md border border-edge px-2 py-1 text-xs text-ink-muted hover:text-ink disabled:opacity-40"
                                        disabled={props.disabled}
                                        onClick={() => void signIn(part.server)}
                                    >
                                        {t("drift.mcp.signIn")}
                                    </button>
                                </Show>
                                <Toggle
                                    label={part.name}
                                    checked={part.enabled}
                                    disabled={props.disabled}
                                    onChange={() => props.onPart("server", part.name, !part.enabled)}
                                />
                            </PartRow>
                        );
                    }}
                </For>
            </div>
        </Show>
    );
}

function ProgramSection(props: { bundle: Bundle; disabled: boolean; onPart: PartChange }) {
    return (
        <Show when={props.bundle.programs.length}>
            <SectionTitle>{t("drift.bundles.section.programs")}</SectionTitle>
            <div class="border-y border-edge/80">
                <PartRow
                    name={t("drift.bundles.programsSwitch")}
                    description={props.bundle.programs.join(", ")}
                    dimmed={!props.bundle.programsEnabled || !props.bundle.enabled}
                >
                    <Toggle
                        label={t("drift.bundles.programsSwitch")}
                        checked={props.bundle.programsEnabled}
                        disabled={props.disabled}
                        onChange={() => props.onPart("programs", "", !props.bundle.programsEnabled)}
                    />
                </PartRow>
            </div>
            <div class="mt-1.5 text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                {t("drift.bundles.programsNote")}
            </div>
        </Show>
    );
}

function HookSection(props: { hooks: string[] }) {
    return (
        <Show when={props.hooks.length}>
            <SectionTitle>{t("drift.bundles.section.hooks")}</SectionTitle>
            <div class="flex flex-wrap gap-1">
                <For each={props.hooks}>{(hook) => <Badge>{hook}</Badge>}</For>
            </div>
            <div class="mt-1.5 text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                {t("drift.bundles.hooksNote")}
            </div>
        </Show>
    );
}

/** One installed bundle: its one switch at the top, then a switch for every part it brought. */
export function BundleScreen(props: {
    bundle: Bundle;
    disabled: boolean;
    confirming: boolean;
    onBack: () => void;
    onEnabled: (enabled: boolean) => void;
    onPart: PartChange;
    onRemove: () => void;
}) {
    const sections = (): [string, BundlePartKind, Bundle["skills"]][] => [
        ["drift.bundles.section.skills", "skill", props.bundle.skills],
        ["drift.bundles.section.agents", "agent", props.bundle.agents],
        ["drift.bundles.section.commands", "command", props.bundle.commands],
    ];

    return (
        <div>
            <BackButton onBack={props.onBack} />
            <div class="mt-4 flex items-start gap-3">
                <LogoTile image={props.bundle.image ?? undefined} title={props.bundle.name} large />
                <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                        <span class="truncate text-base font-semibold text-ink">{props.bundle.name}</span>
                        <Badge tone="accent">{t("drift.bundles.badge")}</Badge>
                    </div>
                    <div class="text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                        {props.bundle.version}
                    </div>
                    <SourceLink source={props.bundle.source} folder={props.bundle.root} />
                </div>
                <RemoveButton disabled={props.disabled} confirming={props.confirming} onRemove={props.onRemove} />
                <Toggle
                    label={props.bundle.name}
                    checked={props.bundle.enabled}
                    disabled={props.disabled}
                    onChange={() => props.onEnabled(!props.bundle.enabled)}
                />
            </div>
            <Show when={!props.bundle.enabled}>
                <div class="mt-3 rounded-md border border-edge bg-raised/30 px-3 py-2 text-xs text-ink-muted">
                    {t("drift.bundles.offNote")}
                </div>
            </Show>
            <For each={sections()}>
                {([title, kind, parts]) => (
                    <PartSection
                        title={t(title)}
                        kind={kind}
                        parts={parts}
                        bundleOn={props.bundle.enabled}
                        disabled={props.disabled}
                        onPart={props.onPart}
                    />
                )}
            </For>
            <ServerSection bundle={props.bundle} disabled={props.disabled} onPart={props.onPart} />
            <ProgramSection bundle={props.bundle} disabled={props.disabled} onPart={props.onPart} />
            <HookSection hooks={props.bundle.hooks} />
        </div>
    );
}

function ContentList(props: { title: string; items: { name: string; description?: string }[] }) {
    return (
        <Show when={props.items.length}>
            <SectionTitle>{props.title}</SectionTitle>
            <div class="border-y border-edge/80">
                <For each={props.items}>
                    {(item) => <PartRow name={item.name} description={item.description} dimmed={false} />}
                </For>
            </div>
        </Show>
    );
}

/** What a registry bundle holds, the values its servers ask for, and Install. */
export function BundleInstallSheet(props: {
    plugin: RegistryPlugin;
    installed: boolean;
    disabled: boolean;
    busy: boolean;
    onBack: () => void;
    onInstall: (values: Record<string, string>) => Promise<void>;
}) {
    const [values, setValues] = createSignal<Record<string, string>>({});
    const contents = () => props.plugin.contents;
    const asks = () => bundleAsks(contents());
    const servers = () =>
        (contents()?.servers ?? []).map((server) => ({ name: server.name, description: server.type }));

    return (
        <div>
            <BackButton onBack={props.onBack} />
            <div class="mt-4 flex items-start gap-3">
                <LogoTile image={props.plugin.image} title={props.plugin.name} large />
                <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                        <span class="text-base font-semibold text-ink">{props.plugin.name}</span>
                        <Badge tone="accent">{t("drift.bundles.badge")}</Badge>
                    </div>
                    <div class="text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                        {props.plugin.author} · v{props.plugin.version}
                    </div>
                    <div class="mt-2 text-sm text-ink-muted">{props.plugin.description}</div>
                    <SourceLink source={props.plugin.source} folder={`${bundlesFolder}/${props.plugin.id}/`} />
                </div>
            </div>
            <Show when={bundleRunsCode(contents())}>
                <div role="note" class="mt-4 rounded-md border border-warn/35 bg-warn/10 px-3 py-2 text-xs text-warn">
                    {t("drift.bundles.runsCode")}
                </div>
            </Show>
            <ContentList title={t("drift.bundles.section.skills")} items={contents()?.skills ?? []} />
            <ContentList title={t("drift.bundles.section.agents")} items={contents()?.agents ?? []} />
            <ContentList title={t("drift.bundles.section.commands")} items={contents()?.commands ?? []} />
            <ContentList title={t("drift.bundles.section.servers")} items={servers()} />
            <Show when={contents()?.programs.length}>
                <SectionTitle>{t("drift.bundles.section.programs")}</SectionTitle>
                <div class="font-mono text-xs text-ink-muted">{contents()?.programs.join(", ")}</div>
            </Show>
            <HookSection hooks={contents()?.hooks ?? []} />
            <Show when={asks().length}>
                <SectionTitle>{t("drift.bundles.values")}</SectionTitle>
                <div class="space-y-2">
                    <For each={asks()}>
                        {(name) => (
                            <label class="flex items-center gap-3">
                                <span class="w-40 shrink-0 truncate font-mono text-xs text-ink-muted">{name}</span>
                                <input
                                    type="password"
                                    autocomplete="off"
                                    aria-label={name}
                                    class="h-8 min-w-0 flex-1 rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none focus:border-accent"
                                    value={values()[name] ?? ""}
                                    onInput={(event) => setValues({ ...values(), [name]: event.currentTarget.value })}
                                />
                            </label>
                        )}
                    </For>
                </div>
                <div class="mt-1.5 text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                    {t("drift.bundles.valuesHint")}
                </div>
            </Show>
            <div class="mt-4 text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                {t("drift.bundles.note", { folder: `${bundlesFolder}/${props.plugin.id}` })}
            </div>
            <div class="mt-3 flex items-center justify-end gap-2">
                <button
                    class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink"
                    onClick={() => props.onBack()}
                >
                    {t("common.cancel")}
                </button>
                <button
                    class="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
                    disabled={props.disabled || props.busy}
                    onClick={() => void props.onInstall(values())}
                >
                    {props.installed ? <IconCheck class="size-3.5" /> : <IconPlus class="size-3.5" />}
                    {t(bundleInstallLabel(props.busy, props.installed))}
                </button>
            </div>
        </div>
    );
}

function bundleInstallLabel(busy: boolean, installed: boolean) {
    if (busy) return "drift.plugins.installing";
    if (installed) return "drift.skills.reinstall";

    return "drift.plugins.install";
}
