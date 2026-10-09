import { classifyMarkdownLink } from "./markdown-links";
import { resolveAttachmentKind } from "../attachments";
import { openWorkspaceFile } from "./markdown";
import { Match, Show, Switch } from "solid-js";
import { openLightbox } from "./lightbox";
import { t } from "../state/i18n";

import type { FilePart } from "../engine/parts";

export function FilePartView(props: { part: Pick<FilePart, "mime" | "name" | "url" | "path">; directory?: string }) {
    const linkable = () => props.part.url.startsWith("data:") || props.part.url.startsWith("http");
    const resolved = () => resolveAttachmentKind({ filename: props.part.name, mime: props.part.mime });
    const kind = () => resolved().kind;
    const mention = () => (props.directory ? props.part.path : undefined);

    return (
        <Show when={mention()} fallback={<AttachmentView part={props.part} linkable={linkable()} kind={kind()} />}>
            {(path) => <MentionChip path={path()} directory={props.directory!} kind={kind()} />}
        </Show>
    );
}

/** An `@` mention: opens the file it names, as a file link in a reply would. */
function MentionChip(props: { path: string; directory: string; kind: string }) {
    const open = () => {
        const link = classifyMarkdownLink(props.path, props.directory);
        if (link.kind === "file") void openWorkspaceFile(link, props.directory);
    };
    return (
        <button
            type="button"
            title={props.path}
            class="inline-flex max-w-full items-center gap-2 rounded-md border border-edge bg-raised py-1 pr-2 pl-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
            onClick={open}
        >
            <AttachmentFileLabel filename={props.path} kind={props.kind === "unsupported" ? "text" : props.kind} bare />
        </button>
    );
}

function AttachmentView(props: { part: Pick<FilePart, "mime" | "name" | "url">; linkable: boolean; kind: string }) {
    const linkable = () => props.linkable;
    const kind = () => props.kind;
    return (
        <Switch
            fallback={
                <Show
                    when={linkable()}
                    fallback={
                        <span class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted">
                            <span class="truncate">{props.part.name ?? t("common.attachment")}</span>
                        </span>
                    }
                >
                    <a
                        href={props.part.url}
                        download={props.part.name ?? "attachment"}
                        title={t("drift.attachment.download")}
                        class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                    >
                        <span class="truncate">{props.part.name ?? t("common.attachment")}</span>
                    </a>
                </Show>
            }
        >
            <Match when={kind() === "image" && linkable()}>
                <ImageThumb url={props.part.url} filename={props.part.name} mime={props.part.mime} />
            </Match>
            <Match when={kind() === "audio" && linkable()}>
                <audio controls src={props.part.url} class="max-w-full" />
            </Match>
            <Match when={kind() === "video" && linkable()}>
                <video controls src={props.part.url} class="max-h-64 max-w-full rounded-lg border border-edge" />
            </Match>
            <Match when={(kind() === "pdf" || kind() === "text" || kind() === "csv") && kind()}>
                {(attachmentKind) => (
                    <Show
                        when={linkable()}
                        fallback={<AttachmentFileLabel filename={props.part.name} kind={attachmentKind()} />}
                    >
                        <a
                            href={props.part.url}
                            download={props.part.name ?? "attachment"}
                            title={t("drift.attachment.download")}
                            class="inline-flex max-w-full items-center gap-2 rounded-md border border-edge bg-raised py-1 pr-2 pl-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                        >
                            <AttachmentFileLabel filename={props.part.name} kind={attachmentKind()} bare />
                        </a>
                    </Show>
                )}
            </Match>
        </Switch>
    );
}

function AttachmentFileLabel(props: { filename?: string; kind: string; bare?: boolean }) {
    return (
        <span
            class="inline-flex min-w-0 max-w-full items-center gap-2"
            classList={{
                "rounded-md border border-edge bg-raised py-1 pr-2 pl-1.5 text-xs text-ink-muted": !props.bare,
            }}
        >
            <span class="rounded bg-overlay px-1 py-0.5 font-mono text-[0.6rem] font-semibold text-accent uppercase">
                {t(`drift.attachment.kind.${props.kind}`)}
            </span>
            <span class="truncate">{props.filename ?? t("common.attachment")}</span>
        </span>
    );
}

function ImageThumb(props: { url: string; filename?: string; mime?: string }) {
    return (
        <button
            title={props.filename ?? t("drift.attachment.viewImage")}
            class="block overflow-hidden rounded-md border border-edge transition-colors hover:border-edge-strong"
            onClick={() => openLightbox({ url: props.url, filename: props.filename, mime: props.mime })}
        >
            <img src={props.url} alt={props.filename ?? t("drift.attachment.image")} class="size-20 object-cover" />
        </button>
    );
}
