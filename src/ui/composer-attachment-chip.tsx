import { formatAttachmentBytes, resolveAttachmentKind } from "../attachments";
import { openLightbox } from "./lightbox";
import { t } from "../state/i18n";
import { IconX } from "./icons";
import { Show } from "solid-js";

import type { StagedFile } from "../state/composer";

/** A staged upload above the composer: an image thumbnail, or its kind, name and size. */
export function AttachmentChip(props: { file: StagedFile; remove: () => void }) {
    const kind = () => resolveAttachmentKind(props.file).kind;
    const label = () => t(`drift.attachment.kind.${kind()}`);
    const detail = () => {
        if (props.file.status === "processing") return t("drift.attachment.processing");
        if (kind() === "text" && props.file.meta.lines !== undefined)
            return t("drift.attachment.lines", { count: props.file.meta.lines });
        if (kind() === "csv" && props.file.meta.rows !== undefined)
            return t("drift.attachment.table", { rows: props.file.meta.rows, columns: props.file.meta.columns ?? 0 });
        if (kind() === "pdf" && props.file.meta.pages !== undefined)
            return t("drift.attachment.pages", { count: props.file.meta.pages });
        return formatAttachmentBytes(props.file.size);
    };
    const title = () => [props.file.filename, props.file.meta.preview].filter(Boolean).join("\n\n");
    const remove = (
        <button
            title={t("prompt.attachment.remove")}
            class="flex size-4 shrink-0 items-center justify-center rounded text-ink-faint hover:bg-overlay hover:text-ink"
            onClick={() => props.remove()}
        >
            <IconX class="size-3" />
        </button>
    );

    return (
        <Show
            when={kind() === "image" && props.file.dataUrl}
            fallback={
                <span
                    class="group/chip flex max-w-64 items-center gap-2 rounded-md border border-edge bg-raised py-1 pr-1 pl-1.5 text-xs text-ink-muted"
                    title={title()}
                >
                    <Show when={kind() === "pdf" && props.file.meta.thumbnail}>
                        {(thumbnail) => (
                            <img src={thumbnail()} alt="" class="h-10 w-8 rounded-sm border border-edge object-cover" />
                        )}
                    </Show>
                    <span class="rounded bg-overlay px-1 py-0.5 font-mono text-[0.6rem] font-semibold text-accent uppercase">
                        {label()}
                    </span>
                    <span class="min-w-0">
                        <span class="block truncate">{props.file.filename}</span>
                        <span class="block truncate text-[0.65rem] text-ink-faint">{detail()}</span>
                    </span>
                    {remove}
                </span>
            }
        >
            {(url) => (
                <div class="group/chip relative">
                    <img
                        src={url()}
                        alt={props.file.filename}
                        title={props.file.filename}
                        class="size-16 cursor-pointer rounded-md border border-edge object-cover transition-colors hover:border-edge-strong"
                        onClick={() =>
                            openLightbox({ url: url(), filename: props.file.filename, mime: props.file.mime })
                        }
                    />
                    <div class="pointer-events-none absolute right-0 bottom-0 left-0 rounded-b-md bg-black/50 px-1 py-0.5">
                        <span class="block truncate text-[0.6rem] text-white">{props.file.filename}</span>
                    </div>
                    <button
                        title={t("prompt.attachment.remove")}
                        class="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full border border-edge bg-overlay text-ink-muted opacity-0 transition-opacity group-hover/chip:opacity-100 hover:bg-raised hover:text-ink"
                        onClick={() => props.remove()}
                    >
                        <IconX class="size-3" />
                    </button>
                </div>
            )}
        </Show>
    );
}
