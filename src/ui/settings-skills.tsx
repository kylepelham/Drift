import { loadRegistrySources, registrySources, sourcesOf } from "../state/registry-sources";
import { createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js";
import { IconCheck, IconSearch, IconSliders, IconTrash } from "./icons";
import { PackSheet, skillsFolder } from "./settings-skill-pack";
import { RegistrySourcesSheet } from "./registry-sources";
import { activeWorkspace } from "../state/workspaces";
import { Badge, Tab } from "./settings-plugins";
import { Chevron, Toggle } from "./controls";
import { LogoTile } from "./logo-tile";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    loadRegistries,
    matchesRegistryQuery,
    type RegistryFailure,
    type RegistryPlugin,
} from "../state/plugin-registry";

import type { SkillPack, UserSkill } from "../engine/native/client";

type View = "installed" | "registry";

/** Manages installed skills and installation of skill packs from registries. */
export function SkillsSection() {
    const engine = useEngine();
    const [view, setView] = createSignal<View>("installed");
    const [skills, setSkills] = createSignal<UserSkill[]>([]);
    const [packs, setPacks] = createSignal<SkillPack[]>([]);
    const [loading, setLoading] = createSignal(false);
    const [busy, setBusy] = createSignal("");
    const [failure, setFailure] = createSignal("");
    const [message, setMessage] = createSignal("");
    const [confirmRemove, setConfirmRemove] = createSignal("");
    const locked = () => engine.state.connection !== "online" || loading() || !!busy();
    const here = () => activeWorkspace()?.path;

    const run = async (action: () => Promise<void>, success?: string) => {
        setLoading(true);
        setFailure("");
        setMessage("");

        try {
            await action();
            if (success) setMessage(success);
            return true;
        } catch (error) {
            setFailure(error instanceof Error ? error.message : String(error));
            return false;
        } finally {
            setLoading(false);
        }
    };

    const refresh = async () => {
        const [list, installed] = await Promise.all([engine.actions.skills(here()), engine.actions.skillPacks()]);

        setSkills(list);
        setPacks(installed);
    };
    onMount(() => void run(refresh));

    const install = async (pack: RegistryPlugin, chosen: string[]) => {
        setBusy(pack.id);

        const done = await run(
            async () => {
                await engine.actions.installSkillPack({
                    id: pack.id,
                    name: pack.name,
                    archive: pack.archive ?? "",
                    subdirs: pack.subdirs ?? [],
                    source: pack.source,
                    image: pack.image,
                    skills: chosen,
                    registry: pack.sourceId,
                });
                await refresh();
            },
            t("drift.skills.installed", { name: pack.name, count: chosen.length || pack.skills?.length || 0 }),
        );

        setBusy("");
        if (done) setView("installed");
        return done;
    };

    const removePack = async (pack: SkillPack) => {
        if (confirmRemove() !== pack.id) return setConfirmRemove(pack.id);

        setBusy(pack.id);
        await run(
            async () => {
                await engine.actions.removeSkillPack(pack.id);
                await refresh();
            },
            t("drift.plugins.removed", { name: pack.name }),
        );
        setBusy("");
        setConfirmRemove("");
    };

    const toggleAll = (list: UserSkill[], on: boolean) => {
        setBusy("all");
        void run(async () => {
            for (const skill of list.filter((skill) => skill.enabled !== on)) {
                await engine.actions.setSkillEnabled(skill.path, on, skill.workspace ? here() : undefined);
            }
            setSkills(await engine.actions.skills(here()));
        }).finally(() => setBusy(""));
    };
    const toggle = (skill: UserSkill) => {
        setBusy(skill.path);
        void run(async () => {
            setSkills(
                await engine.actions.setSkillEnabled(skill.path, !skill.enabled, skill.workspace ? here() : undefined),
            );
        }).finally(() => setBusy(""));
    };

    const groups = createMemo(() => {
        const byPack = new Map<string, UserSkill[]>();

        for (const skill of skills()) {
            const key = skill.pack ?? "";
            byPack.set(key, [...(byPack.get(key) ?? []), skill]);
        }

        const named = packs().map((pack) => ({ pack, skills: byPack.get(pack.id) ?? [] }));
        const loose = byPack.get("") ?? [];

        return {
            named,
            project: loose.filter((skill) => skill.workspace),
            own: loose.filter((skill) => !skill.workspace),
        };
    });
    const installedIds = createMemo(() => new Set(packs().map((pack) => pack.id)));

    return (
        <div class="space-y-3">
            <div class="flex items-center justify-between gap-3">
                <div class="flex rounded-lg border border-edge bg-surface p-0.5">
                    <Tab active={view() === "installed"} onClick={() => setView("installed")}>
                        {t("drift.plugins.tab.installed")}
                    </Tab>
                    <Tab active={view() === "registry"} onClick={() => setView("registry")}>
                        {t("drift.plugins.tab.registry")}
                    </Tab>
                </div>
                <Show when={view() === "installed"}>
                    <button
                        class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                        disabled={locked()}
                        onClick={() => void run(refresh)}
                    >
                        {t("drift.plugins.reload")}
                    </button>
                </Show>
            </div>
            <Show when={failure() || message()}>
                <div
                    role={failure() ? "alert" : "status"}
                    class="rounded-md border px-3 py-2 text-xs"
                    classList={{
                        "border-danger/35 bg-danger/10 text-danger": !!failure(),
                        "border-ok/35 bg-ok/10 text-ok": !failure(),
                    }}
                >
                    {failure() || message()}
                </div>
            </Show>
            <Show when={view() === "installed"}>
                <div class="border-y border-edge/80" aria-busy={loading()}>
                    <For each={groups().own}>
                        {(skill) => (
                            <SkillRow
                                skill={skill}
                                disabled={locked() && busy() !== skill.path}
                                onToggle={() => toggle(skill)}
                            />
                        )}
                    </For>
                    <For each={groups().project}>
                        {(skill) => (
                            <SkillRow
                                skill={skill}
                                badge={activeWorkspace()?.name}
                                disabled={locked() && busy() !== skill.path}
                                onToggle={() => toggle(skill)}
                            />
                        )}
                    </For>
                    <Show when={!loading() && !groups().own.length && !groups().project.length}>
                        <div class="px-3 py-5 text-sm text-ink-faint">
                            {t("drift.skills.empty", { folder: skillsFolder })}
                        </div>
                    </Show>
                </div>
                <Show when={groups().named.length}>
                    <div class="pt-5 pb-1.5 text-[0.68rem] font-semibold tracking-wide text-ink-faint uppercase">
                        {t("drift.skills.packs")}
                    </div>
                    <div class="border-y border-edge/80">
                        <For each={groups().named}>
                            {(group) => (
                                <SkillGroup
                                    title={group.pack.name}
                                    image={group.pack.image ?? undefined}
                                    skills={group.skills}
                                    disabled={locked()}
                                    busy={busy()}
                                    onToggle={toggle}
                                    onToggleAll={(on) => toggleAll(group.skills, on)}
                                    action={
                                        <button
                                            type="button"
                                            disabled={locked()}
                                            title={
                                                confirmRemove() === group.pack.id
                                                    ? t("drift.plugins.confirmRemove")
                                                    : t("drift.plugins.remove")
                                            }
                                            aria-label={
                                                confirmRemove() === group.pack.id
                                                    ? t("drift.plugins.confirmRemove")
                                                    : t("drift.plugins.remove")
                                            }
                                            class="flex items-center gap-1 rounded-md border border-danger/40 px-2 py-1 text-xs text-danger hover:bg-danger/10 disabled:opacity-40"
                                            onClick={(event) => {
                                                event.stopPropagation();
                                                void removePack(group.pack);
                                            }}
                                        >
                                            {confirmRemove() === group.pack.id ? (
                                                t("drift.plugins.confirmRemove")
                                            ) : (
                                                <IconTrash class="size-3.5" />
                                            )}
                                        </button>
                                    }
                                />
                            )}
                        </For>
                    </div>
                </Show>
            </Show>
            <Show when={view() === "registry"}>
                <SkillRegistry installed={installedIds()} disabled={locked()} busy={busy()} onInstall={install} />
            </Show>
        </div>
    );
}

/** Shows a collapsed skill group with a dimmed switch when only some skills are enabled. */
function SkillGroup(props: {
    title: string;
    image?: string;
    skills: UserSkill[];
    disabled: boolean;
    busy: string;
    action?: JSX.Element;
    onToggle: (skill: UserSkill) => void;
    onToggleAll: (on: boolean) => void;
}) {
    const [open, setOpen] = createSignal(false);
    const on = () => props.skills.filter((skill) => skill.enabled).length;
    const all = () => on() === props.skills.length;
    const mixed = () => on() > 0 && !all();

    return (
        <div class="border-b border-edge/70 last:border-b-0">
            <div
                class="flex min-h-13 cursor-pointer items-center gap-3 px-3 py-2.5 hover:bg-raised/40"
                role="button"
                aria-expanded={open()}
                onClick={() => setOpen(!open())}
            >
                <Chevron open={open()} />
                <LogoTile image={props.image} title={props.title} />
                <div class="min-w-0 flex-1">
                    <div class="truncate text-sm font-medium text-ink">{props.title}</div>
                    <div class="text-xs text-ink-faint">
                        {t("drift.skills.packCount", { on: on(), count: props.skills.length })}
                    </div>
                </div>
                {props.action}
                <span classList={{ "opacity-60": mixed() }} title={mixed() ? t("drift.skills.mixed") : undefined}>
                    <Toggle
                        label={props.title}
                        checked={on() > 0}
                        disabled={props.disabled}
                        onChange={() => props.onToggleAll(!all())}
                    />
                </span>
            </div>
            <Show when={open()}>
                <div class="border-t border-edge/70 bg-raised/15 pl-6">
                    <For each={props.skills}>
                        {(skill) => (
                            <SkillRow
                                skill={skill}
                                disabled={props.disabled && props.busy !== skill.path}
                                onToggle={() => props.onToggle(skill)}
                            />
                        )}
                    </For>
                </div>
            </Show>
        </div>
    );
}

function SkillRow(props: { skill: UserSkill; badge?: string; disabled: boolean; onToggle: () => void }) {
    return (
        <div
            class="flex min-h-11 cursor-pointer items-center gap-3 border-b border-edge/70 px-3 py-2 last:border-b-0 hover:bg-raised/40"
            classList={{ "opacity-60": !props.skill.enabled }}
            onClick={() => !props.disabled && props.onToggle()}
        >
            <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                    <span class="truncate text-[0.82rem] font-medium text-ink">{props.skill.name}</span>
                    <Show when={props.badge}>{(name) => <Badge>{name()}</Badge>}</Show>
                </div>
                <Show when={props.skill.description}>
                    <div class="truncate text-xs text-ink-faint" title={props.skill.description}>
                        {props.skill.description}
                    </div>
                </Show>
            </div>
            <Toggle
                label={props.skill.name}
                checked={props.skill.enabled}
                disabled={props.disabled}
                onChange={props.onToggle}
            />
        </div>
    );
}

function SkillRegistry(props: {
    installed: Set<string>;
    disabled: boolean;
    busy: string;
    onInstall: (pack: RegistryPlugin, skills: string[]) => Promise<boolean>;
}) {
    const engine = useEngine();
    const [query, setQuery] = createSignal("");
    const [packs, setPacks] = createSignal<RegistryPlugin[]>([]);
    const [failures, setFailures] = createSignal<RegistryFailure[]>([]);
    const [loading, setLoading] = createSignal(true);
    const [error, setError] = createSignal("");
    const [selected, setSelected] = createSignal<RegistryPlugin>();
    const [sourcesOpen, setSourcesOpen] = createSignal(false);

    const load = async (fresh = false) => {
        setLoading(true);
        setError("");

        try {
            await loadRegistrySources({
                settings: () => engine.actions.engineSettings(),
                putSettings: (body) => engine.actions.putEngineSettings(body),
            }).catch(() => undefined);

            const loaded = await loadRegistries(sourcesOf("plugins"), fresh, (id) => engine.actions.fetchRegistry(id));
            setPacks(loaded.plugins.filter((plugin) => plugin.kind === "skill" || plugin.kind === "skills"));
            setFailures(loaded.failures);
            if (!loaded.plugins.length && loaded.failures.length) setError(t("drift.plugins.registryLoadFailed"));
        } catch {
            setError(t("drift.plugins.registryLoadFailed"));
        } finally {
            setLoading(false);
        }
    };
    onMount(() => void load());
    const closeSources = () => {
        setSourcesOpen(false);
        void load(true);
    };
    const visible = createMemo(() =>
        packs().filter(
            (pack) =>
                matchesRegistryQuery(pack, query()) ||
                pack.skills?.some((skill) => skill.name.toLowerCase().includes(query().trim().toLowerCase())),
        ),
    );

    async function installSelected(pack: RegistryPlugin, chosen: string[]) {
        if (await props.onInstall(pack, chosen)) setSelected();
    }

    return (
        <Show when={!sourcesOpen()} fallback={<RegistrySourcesSheet kind="plugins" onBack={closeSources} />}>
            <Show
                when={selected()}
                fallback={
                    <div class="space-y-3">
                        <div class="flex flex-wrap items-center gap-2">
                            <label class="flex h-9 min-w-48 flex-1 items-center gap-2 rounded-md border border-edge bg-raised/45 px-2.5 focus-within:border-accent">
                                <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                                <input
                                    aria-label={t("drift.skills.registrySearch")}
                                    placeholder={t("drift.skills.registrySearch")}
                                    class="min-w-0 flex-1 bg-transparent text-sm text-ink outline-none placeholder:text-ink-faint"
                                    value={query()}
                                    onInput={(event) => setQuery(event.currentTarget.value)}
                                />
                            </label>
                            <button
                                type="button"
                                title={t("drift.registry.sources")}
                                aria-label={t("drift.registry.sources")}
                                class="flex h-9 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted hover:border-edge-strong hover:text-ink"
                                onClick={() => setSourcesOpen(true)}
                            >
                                <IconSliders class="size-3.5" />
                                <Show when={sourcesOf("plugins").length}>{(count) => <span>{count()}</span>}</Show>
                            </button>
                        </div>
                        <div class="text-[0.7rem] text-ink-faint">
                            {t(
                                registrySources().some((source) => source.kind === "plugins")
                                    ? "drift.skills.registrySourceWithOwn"
                                    : "drift.skills.registrySource",
                            )}
                        </div>
                        <For each={failures()}>
                            {(failure) => (
                                <div
                                    role="alert"
                                    class="rounded-md border border-warn/35 bg-warn/10 px-3 py-2 text-xs text-warn"
                                >
                                    {t("drift.registry.sources.failed", { name: failure.name, error: failure.error })}
                                </div>
                            )}
                        </For>
                        <Show when={error()}>
                            <div
                                role="alert"
                                class="flex items-center gap-3 rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-xs text-danger"
                            >
                                {error()}
                                <button
                                    class="rounded border border-current px-2 py-0.5"
                                    onClick={() => void load(true)}
                                >
                                    {t("drift.mcp.retry")}
                                </button>
                            </div>
                        </Show>
                        <Show when={loading() && !packs().length}>
                            <div class="grid gap-2 sm:grid-cols-2" aria-busy="true">
                                <For each={Array.from({ length: 4 })}>
                                    {() => (
                                        <div class="h-[6.5rem] animate-pulse rounded-lg border border-edge bg-raised/30" />
                                    )}
                                </For>
                            </div>
                        </Show>
                        <div class="grid gap-2 sm:grid-cols-2" aria-busy={loading()}>
                            <For each={visible()}>
                                {(pack) => (
                                    <button
                                        type="button"
                                        class="group flex min-w-0 flex-col gap-2 rounded-lg border border-edge bg-surface p-3 text-left transition-colors hover:border-edge-strong hover:bg-raised/40 focus-visible:border-accent focus-visible:outline-none"
                                        onClick={() => setSelected(pack)}
                                    >
                                        <div class="flex min-w-0 items-start gap-2.5">
                                            <LogoTile image={pack.image} title={pack.name} />
                                            <div class="min-w-0 flex-1">
                                                <div class="flex items-center gap-1.5">
                                                    <span class="truncate text-sm font-medium text-ink">
                                                        {pack.name}
                                                    </span>
                                                    <Show when={props.installed.has(pack.id)}>
                                                        <IconCheck
                                                            class="size-3.5 shrink-0 text-ok"
                                                            aria-label={t("drift.plugins.installedLabel")}
                                                        />
                                                    </Show>
                                                </div>
                                                <div class="truncate text-[0.7rem] text-ink-faint">{pack.author}</div>
                                            </div>
                                        </div>
                                        <div class="line-clamp-2 text-xs leading-relaxed text-ink-muted">
                                            {pack.description}
                                        </div>
                                        <div class="mt-auto flex flex-wrap gap-1">
                                            <Show when={pack.sourceName}>
                                                {(name) => <Badge tone="warn">{name()}</Badge>}
                                            </Show>
                                            <Badge tone="accent">
                                                {pack.kind === "skill"
                                                    ? t("drift.skills.single")
                                                    : t("drift.skills.count", { count: pack.skills?.length ?? 0 })}
                                            </Badge>
                                        </div>
                                    </button>
                                )}
                            </For>
                        </div>
                        <Show when={!loading() && !error() && !visible().length}>
                            <div class="px-3 py-6 text-center text-sm text-ink-faint">
                                {t("drift.plugins.registryEmpty")}
                            </div>
                        </Show>
                    </div>
                }
            >
                {(pack) => (
                    <PackSheet
                        pack={pack()}
                        installed={props.installed.has(pack().id)}
                        disabled={props.disabled}
                        busy={props.busy === pack().id}
                        onBack={() => setSelected()}
                        onInstall={(chosen) => installSelected(pack(), chosen)}
                    />
                )}
            </Show>
        </Show>
    );
}
