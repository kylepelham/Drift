import { diffIndicator, diffLineNumbers, diffWordWrap, syntaxTheme } from "../state/code";
import { createEffect, createMemo, createSignal, For, Show, untrack } from "solid-js";
import { codeTokens, type SyntaxToken } from "./markdown";
import { resolveFileLanguage } from "../syntax-language";
import { Chevron } from "./controls";
import { t } from "../state/i18n";

import type { PatchFile } from "./tool-labels";

// Shiki packs font styling into a bitmask on each token; these are its FontStyle enum values.
const fontStyleItalic = 1;
const fontStyleBold = 2;
const fontStyleUnderline = 4;

type DiffRow = { kind: "add" | "del" | "ctx" | "gap"; line?: number; text: string };

export function parseDiff(diff: string): DiffRow[] {
    const rows: DiffRow[] = [];
    let oldLine = 0;
    let newLine = 0;
    let oldRemaining = 0;
    let newRemaining = 0;
    let inHunk = false;
    const lines = diff.split("\n");
    if (lines.at(-1) === "") lines.pop();
    for (const line of lines) {
        const hunk = line.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/);
        if (hunk) {
            oldLine = Number(hunk[1]);
            oldRemaining = hunk[2] === undefined ? 1 : Number(hunk[2]);
            newLine = Number(hunk[3]);
            newRemaining = hunk[4] === undefined ? 1 : Number(hunk[4]);
            inHunk = true;
            if (rows.length) rows.push({ kind: "gap", text: "" });
            continue;
        }
        if (!inHunk) continue;
        if (line.startsWith("\\")) continue;
        if (line.startsWith("+")) {
            rows.push({ kind: "add", line: newLine++, text: line.slice(1) });
            newRemaining--;
        } else if (line.startsWith("-")) {
            rows.push({ kind: "del", line: oldLine++, text: line.slice(1) });
            oldRemaining--;
        } else if (line.startsWith(" ")) {
            rows.push({ kind: "ctx", line: newLine, text: line.slice(1) });
            oldLine++;
            newLine++;
            oldRemaining--;
            newRemaining--;
        }
        if (oldRemaining <= 0 && newRemaining <= 0) inHunk = false;
    }
    return rows;
}

/**
 * Identifies one exact highlighting result: the same file, code, language and theme.
 *
 * Keying by content rather than by object identity is what keeps a diff highlighted. Every
 * transcript update rebuilds the parsed rows, and an identity check would treat that equal content
 * as new work, blanking the colours until shiki answered again. An empty key means the language is
 * still resolving, or resolved against a filename this panel no longer shows.
 */
export function diffHighlightKey(
    theme: string,
    language: { filename: string; value: string } | undefined,
    filename: string,
    code: string,
) {
    if (!language || language.filename !== filename) return "";
    return `${theme}\0${language.value}\0${code}`;
}

export function DiffPanel(props: { diff: string; filename: string; bare?: boolean }) {
    const rows = createMemo(() => parseDiff(props.diff));
    const code = createMemo(() =>
        rows()
            .map((row) => row.text)
            .join("\n"),
    );
    const [language, setLanguage] = createSignal<{ filename: string; value: string }>();
    const [highlight, setHighlight] = createSignal<{ key: string; tokens: SyntaxToken[][] }>();
    let languageRequest = 0;
    let request = 0;
    createEffect(() => {
        const filename = props.filename;
        const current = ++languageRequest;
        setLanguage(undefined);
        void resolveFileLanguage(filename)
            // A failed catalog load must degrade to plain text, not leave the panel unhighlighted forever.
            .catch(() => "text")
            .then((value) => {
                if (current === languageRequest) setLanguage({ filename, value });
            });
    });
    const highlightKey = createMemo(() => diffHighlightKey(syntaxTheme(), language(), props.filename, code()));
    createEffect(() => {
        const key = highlightKey();
        const resolved = language();
        const current = ++request;
        if (!key || !resolved) return;
        if (untrack(() => highlight()?.key) === key) return;
        void codeTokens(code(), resolved.value)
            .catch(() => [] as SyntaxToken[][])
            .then((tokens) => {
                if (current === request) setHighlight({ key, tokens });
            });
    });
    const visibleTokens = () => (highlight()?.key === highlightKey() ? (highlight()?.tokens ?? []) : []);
    return (
        <div class="diff-view overflow-hidden" classList={{ "rounded-lg border border-edge": !props.bare }}>
            <div class="transcript-tool-output max-h-80 overflow-auto py-1 font-mono leading-relaxed">
                <div classList={{ "w-full": diffWordWrap(), "w-max min-w-full": !diffWordWrap() }}>
                    <For each={rows()}>
                        {(row, index) => (
                            <Show
                                when={row.kind !== "gap"}
                                fallback={<div class="my-1 border-t border-dashed border-edge" />}
                            >
                                <div
                                    class="flex border-l-2"
                                    classList={{
                                        "bg-ok/10": diffIndicator() === "background" && row.kind === "add",
                                        "bg-danger/10": diffIndicator() === "background" && row.kind === "del",
                                        "border-l-ok": diffIndicator() === "bars" && row.kind === "add",
                                        "border-l-danger": diffIndicator() === "bars" && row.kind === "del",
                                        "border-l-transparent": diffIndicator() !== "bars" || row.kind === "ctx",
                                    }}
                                >
                                    <Show when={diffLineNumbers()}>
                                        <span class="w-10 shrink-0 pr-2 text-right text-ink-faint select-none">
                                            {row.line}
                                        </span>
                                    </Show>
                                    <Show when={diffIndicator() === "symbols"}>
                                        <span
                                            class="w-4 shrink-0 text-center text-ink-faint select-none"
                                            classList={{
                                                "text-ok": row.kind === "add",
                                                "text-danger": row.kind === "del",
                                            }}
                                        >
                                            {diffSymbol(row.kind)}
                                        </span>
                                    </Show>
                                    <span
                                        class="min-w-0 flex-1 pr-3 text-ink-muted"
                                        classList={{
                                            "whitespace-pre-wrap [overflow-wrap:anywhere]": diffWordWrap(),
                                            "whitespace-pre": !diffWordWrap(),
                                        }}
                                    >
                                        <Show
                                            when={
                                                visibleTokens()[index()]?.length ? visibleTokens()[index()] : undefined
                                            }
                                            fallback={
                                                <span
                                                    classList={{
                                                        "text-ok": row.kind === "add",
                                                        "text-danger": row.kind === "del",
                                                    }}
                                                >
                                                    {row.text}
                                                </span>
                                            }
                                        >
                                            {(line) => (
                                                <For each={line()}>{(token) => <SyntaxTokenView token={token} />}</For>
                                            )}
                                        </Show>
                                    </span>
                                </div>
                            </Show>
                        )}
                    </For>
                </div>
            </div>
        </div>
    );
}

function SyntaxTokenView(props: { token: SyntaxToken }) {
    const style = () => {
        const fontStyle = props.token.fontStyle ?? 0;
        return {
            color: props.token.color,
            "font-style": fontStyle & fontStyleItalic ? "italic" : undefined,
            "font-weight": fontStyle & fontStyleBold ? "bold" : undefined,
            "text-decoration": fontStyle & fontStyleUnderline ? "underline" : undefined,
        };
    };
    return <span style={style()}>{props.token.content}</span>;
}

function diffSymbol(kind: DiffRow["kind"]) {
    if (kind === "add") return "+";
    if (kind === "del") return "-";

    return " ";
}

export function PatchPanel(props: { files: PatchFile[] }) {
    return (
        <div class="space-y-2">
            <For each={props.files}>{(file) => <PatchFilePanel file={file} />}</For>
        </div>
    );
}

function PatchFilePanel(props: { file: PatchFile }) {
    const [open, setOpen] = createSignal(true);
    const status = () => {
        if (props.file.type === "add") return t("drift.file.created");
        if (props.file.type === "delete") return t("drift.file.deleted");
        if (props.file.type === "move") return t("drift.file.moved");
        return null;
    };
    return (
        <div class="overflow-hidden rounded-lg border border-edge">
            <button
                class="flex w-full items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-raised/50"
                onClick={() => setOpen(!open())}
            >
                <span class="min-w-0 flex-1 truncate font-mono text-xs text-ink-muted">
                    {props.file.relativePath ?? props.file.filePath}
                </span>
                <span class="shrink-0 font-mono text-xs">
                    <span class="text-ok">+{props.file.additions}</span>{" "}
                    <span class="text-danger">-{props.file.deletions}</span>
                </span>
                <Show when={status()}>{(label) => <span class="shrink-0 text-xs text-ink-faint">{label()}</span>}</Show>
                <Chevron open={open()} />
            </button>
            <Show when={open()}>
                <div class="border-t border-edge">
                    <DiffPanel diff={props.file.patch} filename={props.file.relativePath ?? props.file.filePath} bare />
                </div>
            </Show>
        </div>
    );
}
