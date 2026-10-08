import { toolInput, toolMetadata } from "../engine/parts";
import { agentLabel, t } from "../state/i18n";

import type { EngineState } from "../engine/store";
import type { ToolPart } from "../engine/parts";

export const contextTools = new Set(["read", "glob", "grep", "list"]);
const maxArgsPreviewChars = 120;

type ToolInfo = { title?: string; called?: string; subtitle?: string; mono?: boolean };
export type PatchFile = {
    filePath: string;
    relativePath?: string;
    type?: "add" | "update" | "delete" | "move";
    patch: string;
    additions: number;
    deletions: number;
};

export function toolFilename(path?: string) {
    if (!path) return undefined;

    return path.replaceAll("\\", "/").split("/").filter(Boolean).at(-1);
}

function toolInputText(input: Record<string, unknown>, key: string) {
    return typeof input[key] === "string" ? (input[key] as string) : undefined;
}

function toolCount(value: unknown, truncated: unknown, singular: string, plural: string) {
    if (typeof value !== "number") return "";

    const count = `${value}${truncated ? "+" : ""}`;
    return ` · ${t(value === 1 ? singular : plural, { count })}`;
}

function argsPreview(input: Record<string, unknown>) {
    const parts = Object.entries(input).map(([key, value]) => {
        const raw = typeof value === "string" ? value : JSON.stringify(value);
        return `${key}=${raw}`;
    });
    const joined = parts.join("  ");

    return joined.length > maxArgsPreviewChars ? joined.slice(0, maxArgsPreviewChars) + "..." : joined;
}

export function toolInfo(part: ToolPart): ToolInfo {
    const input = toolInput(part);
    const meta = toolMetadata(part);
    const text = (key: string) => toolInputText(input, key);
    const context = contextToolInfo(part, input, meta);
    if (context) return context;

    switch (part.name) {
        case "bash":
            return { title: t("prompt.mode.shell"), subtitle: text("command"), mono: true };
        case "edit":
        case "write":
            return { title: t("settings.permissions.tool.edit.title"), subtitle: toolFilename(text("filePath")) };
        case "apply_patch":
            return { title: t("settings.permissions.tool.edit.title"), subtitle: patchSubtitle(part) };
        case "webfetch":
            return { title: t("drift.tool.fetch"), subtitle: text("url"), mono: true };
        default:
            return delegatedToolInfo(part, input, meta);
    }
}

function contextToolInfo(
    part: ToolPart,
    input: Record<string, unknown>,
    meta: Record<string, unknown>,
): ToolInfo | undefined {
    const text = (key: string) => toolInputText(input, key);
    const output = () => (part.status === "done" ? (part.output ?? "") : "");
    const count = (value: unknown, singular: string, plural: string) =>
        toolCount(value, meta.truncated, singular, plural);

    switch (part.name) {
        case "read": {
            const lines = output() ? output().split("\n").length : undefined;
            const path = toolFilename(text("filePath")) ?? "";
            const suffix = count(lines, "drift.count.line.one", "drift.count.line.other");

            return { title: t("settings.permissions.tool.read.title"), subtitle: `${path}${suffix}` };
        }
        case "list": {
            const path = toolFilename(text("path")) ?? "";
            const suffix = count(meta.count, "drift.count.entry.one", "drift.count.entry.other");

            return { title: t("settings.permissions.tool.list.title"), subtitle: `${path}${suffix}` };
        }
        case "glob": {
            const pattern = text("pattern") ?? "";
            const suffix = count(meta.count, "drift.count.file.one", "drift.count.file.other");

            return { title: t("settings.permissions.tool.glob.title"), subtitle: `${pattern}${suffix}`, mono: true };
        }
        case "grep": {
            const pattern = text("pattern") ?? "";
            const suffix = count(meta.matches, "drift.count.match.one", "drift.count.match.other");

            return { title: t("settings.permissions.tool.grep.title"), subtitle: `${pattern}${suffix}`, mono: true };
        }
        case "websearch": {
            const results = output() ? output().match(/^#|^\d+\./gm)?.length : undefined;
            const query = text("query") ?? "";
            const suffix = count(results, "drift.count.result.one", "drift.count.result.other");

            return { title: t("common.search.placeholder"), subtitle: `${query}${suffix}` };
        }
    }
}

function delegatedToolInfo(part: ToolPart, input: Record<string, unknown>, meta: Record<string, unknown>): ToolInfo {
    const text = (key: string) => toolInputText(input, key);

    switch (part.name) {
        case "task":
            return { title: taskHeading(text("subagent_type"), text("description")) };
        case "spawn_thread": {
            const title = text("title");

            return { title: title ? `${t("drift.tool.spawn")} ${title}` : t("drift.tool.spawn") };
        }
        case "read_thread":
            return {
                title: t("drift.tool.readThread"),
                subtitle: part.status === "done" ? (part.title ?? part.name) : undefined,
            };
        case "question":
            return questionToolInfo(input, meta);
        case "skill":
            return { title: text("name") ?? t("prompt.slash.badge.skill") };
        default:
            return { called: part.name, subtitle: argsPreview(input), mono: true };
    }
}

function questionToolInfo(input: Record<string, unknown>, meta: Record<string, unknown>): ToolInfo {
    const title =
        meta.async === true || input.async === true ? "drift.tool.asyncQuestion" : "notification.question.title";
    const questions = input.questions as { header?: string }[] | undefined;
    const subtitle = toolInputText(input, "question") ?? questions?.[0]?.header;

    return { title: t(title), subtitle };
}

export function taskHeading(agent?: string, description?: string) {
    const label = agent ? agentLabel(agent) : t("drift.tool.task");

    return description ? `${label} ${description}` : label;
}

export function awaitingPermission(state: EngineState, part: ToolPart) {
    return (
        (state.permissions[part.sessionId] ?? []).some((permission) => permission.callId === part.callId) ||
        (state.questions[part.sessionId] ?? []).some((question) => !question.async && question.callId === part.callId)
    );
}

export function formatShellTimeout(ms: number) {
    if (ms % 60_000 === 0) return `${ms / 60_000}m`;
    if (ms % 1_000 === 0) return `${ms / 1_000}s`;

    return `${ms}ms`;
}

export function shellTimeoutStatus(part: ToolPart) {
    if (part.name !== "bash") return null;

    const metadata = toolMetadata(part);
    if (!("shellTimeoutMs" in metadata)) return null;

    const timeout = metadata.shellTimeoutMs;
    const timedOut = metadata.timedOut === true;
    if (typeof timeout !== "number" || !Number.isFinite(timeout) || timeout <= 0) return null;
    if (!timedOut && part.status !== "running" && part.status !== "pending") return null;

    const duration = formatShellTimeout(timeout);
    const text = timedOut
        ? t("drift.shell.timeout.expired", { duration })
        : t("drift.shell.timeout.limit", { duration });

    return { timedOut, timeoutMs: timeout, text };
}

export function patchFiles(part: ToolPart) {
    const files = toolMetadata(part).files;
    if (!Array.isArray(files)) return [];

    return files.filter(
        (file): file is PatchFile =>
            !!file &&
            typeof file === "object" &&
            typeof (file as PatchFile).filePath === "string" &&
            typeof (file as PatchFile).patch === "string" &&
            typeof (file as PatchFile).additions === "number" &&
            typeof (file as PatchFile).deletions === "number",
    );
}

export function patchSubtitle(part: ToolPart) {
    const files = patchFiles(part);
    if (files.length === 1) return toolFilename(files[0].relativePath ?? files[0].filePath);

    const input = toolInput(part) as { files?: unknown[] };
    const paths = patchInputPaths(part);
    if (!files.length && paths.length === 1) return toolFilename(paths[0]);

    const count = files.length || paths.length || input.files?.length || 0;

    return count ? t(count === 1 ? "drift.count.file.one" : "drift.count.file.other", { count }) : undefined;
}

export function patchInputPaths(part: ToolPart) {
    const patch = toolInput(part).patchText;
    if (typeof patch !== "string") return [];

    return [...patch.matchAll(/^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm)].map((match) => match[1].trim());
}
