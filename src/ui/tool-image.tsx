import { toolInput, toolMetadata } from "../engine/parts";
import { AsyncSizeCache } from "./markdown-syntax";
import { createResource, Show } from "solid-js";
import { toolFilename } from "./tool-labels";
import { openLightbox } from "./lightbox";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { ToolPart } from "../engine/parts";

const imageCacheBudget = 32 * 1024 * 1024;

/** The image a finished `generate_image` call saved, as the engine keeps it, and the file it was saved to. */
export function generatedImage(part: ToolPart) {
    if (part.name !== "generate_image" || part.status !== "done") return null;

    const meta = toolMetadata(part);
    const images = Array.isArray(meta.images) ? (meta.images as { mime?: unknown; hash?: unknown }[]) : [];
    const image = images.find((entry) => typeof entry?.hash === "string");
    if (!image) return null;
    const files = Array.isArray(meta.files) ? meta.files : [];

    return {
        hash: image.hash as string,
        filename: typeof files[0] === "string" ? toolFilename(files[0]) : undefined,
    };
}

/** The picture a finished `read` call showed the model, as the engine keeps it, and the path it was read from. */
export function readImage(part: ToolPart) {
    if (part.name !== "read" || part.status !== "done") return null;

    const input = toolInput(part) as { path?: unknown; filePath?: unknown };
    const path = [input.path, input.filePath].find((value): value is string => typeof value === "string" && !!value);
    const meta = toolMetadata(part);
    const images = Array.isArray(meta.images) ? (meta.images as { mime?: unknown; hash?: unknown }[]) : [];
    // A PDF is kept the same way but cannot be drawn as an image.
    const image = images.find(
        (entry) => typeof entry?.hash === "string" && typeof entry.mime === "string" && entry.mime.startsWith("image/"),
    );
    if (!image || !path) return null;

    return { hash: image.hash as string, path };
}

/** A read image in its expanded row: the path it came from, then the picture, opened large on click. */
export function ReadImage(props: { hash: string; path: string }) {
    return (
        <div class="tool-card space-y-1.5">
            <div class="font-mono text-xs break-all text-ink-faint">{props.path}</div>
            <ToolImage hash={props.hash} filename={toolFilename(props.path)} />
        </div>
    );
}

/** Data URLs by content hash: a picture shown in several rows, or remounted by the virtual list, loads once. */
const imageCache = new AsyncSizeCache<{ url: string; mime: string }>(imageCacheBudget, (image) => image.url.length);

function loadImage(fetch: (hash: string) => Promise<{ mime: string; data: string }>, hash: string) {
    return (
        imageCache.get(hash) ??
        imageCache.set(
            hash,
            0,
            fetch(hash).then((loaded) => ({ url: `data:${loaded.mime};base64,${loaded.data}`, mime: loaded.mime })),
        )
    );
}

/** A generated image under its call's row, opened large on click; it shows once the engine hands it over. */
export function ToolImage(props: { hash: string; filename?: string }) {
    const engine = useEngine();
    const [image] = createResource(
        () => props.hash,
        (hash) => loadImage(engine.actions.toolImage, hash).catch(() => undefined),
    );
    const url = () => image()?.url;

    return (
        <Show when={url()}>
            {(src) => (
                <button
                    title={props.filename ?? t("drift.tool.generatedImage")}
                    class="ms-1 block w-fit overflow-hidden rounded-md border border-edge transition-colors hover:border-edge-strong"
                    onClick={() => openLightbox({ url: src(), filename: props.filename, mime: image()?.mime })}
                >
                    <img
                        src={src()}
                        alt={props.filename ?? t("drift.tool.generatedImage")}
                        class="max-h-64 max-w-full object-contain"
                    />
                </button>
            )}
        </Show>
    );
}
