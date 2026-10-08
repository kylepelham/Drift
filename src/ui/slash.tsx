import { composerScope, draftFromMessage, setComposerDraft } from "../state/composer";
import { previousUserMessage, resolveModel, savedChoice } from "../engine/store";
import { activeWorkspace, archiveSession } from "../state/workspaces";
import { selectedSession, selectSession } from "../state/selection";
import { setTheme, theme, themes } from "../state/theme";
import { emitThreadArchived } from "../plugins";
import { archiveFailed } from "./workspaces";
import { restoreReverted } from "./revert";
import { prefsFor } from "../state/prefs";
import { openMcpServers } from "./mcp";
import { t } from "../state/i18n";

import type { Engine } from "../engine";

export type SlashPreset = {
    value: string;
    label: string;
    description: string;
    usage?: string;
    execute?: boolean;
    literal?: boolean;
};
export type SlashItem = {
    name: string;
    description: string;
    needsSession?: boolean;
    engine?: boolean;
    requiredArgs?: boolean;
    usage?: string;
    presets?: SlashPreset[];
};

const builtins: SlashItem[] = [
    { name: "new", description: "command.session.new" },
    {
        name: "fork",
        description: "drift.slash.fork",
        needsSession: true,
        usage: "[active|all]",
        presets: [
            {
                value: "active",
                label: "drift.slash.fork.active",
                description: "drift.slash.fork.active.description",
                execute: true,
            },
            {
                value: "all",
                label: "drift.slash.fork.all",
                description: "drift.slash.fork.all.description",
                execute: true,
            },
        ],
    },
    {
        name: "spawn",
        description: "drift.slash.spawn",
        needsSession: true,
        requiredArgs: true,
        usage: "<instruction>",
    },
    { name: "archive", description: "command.session.archive", needsSession: true },
    { name: "undo", description: "command.session.undo.description", needsSession: true },
    { name: "redo", description: "command.session.redo.description", needsSession: true },
    { name: "compact", description: "command.session.compact.description", needsSession: true },
    { name: "theme", description: "command.theme.cycle" },
    { name: "mcp", description: "drift.slash.mcp" },
];

export function parseSlash(draft: string) {
    if (!draft.startsWith("/") || draft.startsWith("//")) return null;
    const body = draft.slice(1);
    const space = body.search(/\s/);
    if (space < 0) return { query: body, args: "", separated: false };
    return { query: body.slice(0, space), args: body.slice(space + 1).trim(), separated: true };
}

export function slashItems(engine: Engine, query: string): SlashItem[] {
    const needle = query.toLowerCase();
    const engineItems: SlashItem[] = engine.state.commands.map((command) => ({
        name: command.name,
        description: command.description ?? t("drift.slash.workspaceCommand"),
        engine: true,
        usage: command.usage,
        presets: command.subcommands?.map((subcommand) => ({
            value: `${subcommand.name} `,
            label: subcommand.name,
            description: subcommand.description,
            usage: subcommand.usage,
            literal: true,
        })),
    }));
    return [...builtins.map((item) => ({ ...item, description: t(item.description) })), ...engineItems]
        .filter((item) => !item.needsSession || selectedSession())
        .filter((item) => item.name.toLowerCase().startsWith(needle));
}

export function slashItem(engine: Engine, name: string) {
    return slashItems(engine, name).find((item) => item.name.toLowerCase() === name.toLowerCase());
}

export function slashPresets(item: SlashItem, query: string) {
    const value = query.toLowerCase();
    return (item.presets ?? [])
        .map((preset) =>
            preset.literal ? preset : { ...preset, label: t(preset.label), description: t(preset.description) },
        )
        .filter((preset) => !value || preset.value.trim().toLowerCase().startsWith(value));
}

export async function runSlash(engine: Engine, item: SlashItem, args: string) {
    const current = selectedSession();
    if (item.engine) {
        const id = current ?? (await engine.actions.newSession())?.id;
        if (!id) return;
        selectSession(id);
        return engine.actions.runCommand(id, item.name, args);
    }
    if (item.name === "new") return selectSession(null);
    if (item.name === "theme") setTheme(themes[(themes.indexOf(theme()) + 1) % themes.length]);
    if (item.name === "mcp") openMcpServers();
    if (current) return runSessionSlash(engine, current, item.name, args);
}

function runSessionSlash(engine: Engine, current: string, name: string, args: string) {
    switch (name) {
        case "fork":
            return forkSession(engine, current, args);
        case "spawn":
            return spawnSession(engine, current, args);
        case "archive":
            return archiveCurrentSession(engine, current);
        case "compact":
            return engine.actions.summarize(
                current,
                resolveModel(engine.state, prefsFor(current, savedChoice(engine.state, current)).model),
            );
        case "undo":
            return undoCurrentSession(engine, current);
        case "redo": {
            const marker = engine.state.sessions[current]?.revert?.messageId;
            if (marker) return restoreReverted(engine, current, marker);
        }
    }
}

async function forkSession(engine: Engine, current: string, args: string) {
    const mode = args.toLowerCase() || "active";
    if (mode !== "active" && mode !== "all") {
        engine.actions.notice({ message: t("drift.slash.fork.invalid"), variant: "warning" });
        return;
    }

    const session = await engine.actions.fork(current);
    if (session && selectedSession() === current) selectSession(session.id);
}

async function spawnSession(engine: Engine, current: string, args: string) {
    if (!args.trim()) {
        engine.actions.notice({ message: t("drift.slash.spawn.required"), variant: "warning" });
        return;
    }

    const session = await engine.actions.spawn(current, args.trim());
    if (session && selectedSession() === current) selectSession(session.id);
}

function archiveCurrentSession(engine: Engine, current: string) {
    const workspace = activeWorkspace();
    if (!workspace) return;

    selectSession(null);
    return archiveSession(current, workspace.id, engine.actions.setArchived)
        .then(() => emitThreadArchived(current))
        .catch((cause: unknown) => archiveFailed(engine, cause));
}

async function undoCurrentSession(engine: Engine, current: string) {
    const marker = engine.state.sessions[current]?.revert?.messageId;
    const target = previousUserMessage(engine.state.transcripts[current] ?? [], marker ?? undefined);
    if (!target) return;

    const restored = draftFromMessage(target);
    if (await engine.actions.revert(current, target.info.id)) setComposerDraft(composerScope(current), restored);
}
