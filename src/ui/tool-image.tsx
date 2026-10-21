import { toolInput, toolMetadata } from "../engine/parts";
import { createResource, Show } from "solid-js";
import { toolFilename } from "./tool-labels";
import { openLightbox } from "./lightbox";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { ToolPart } from "../engine/parts";

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
        <div class="space-y-1.5 border-l-2 border-edge pl-3">
            <div class="font-mono text-xs break-all text-ink-faint">{props.path}</div>
            <ToolImage hash={props.hash} filename={toolFilename(props.path)} />
        </div>
    );
}

/** A generated image under its call's row, opened large on click; it shows once the engine hands it over. */
export function ToolImage(props: { hash: string; filename?: string }) {
    const engine = useEngine();
    const [image] = createResource(
        () => props.hash,
        (hash) => engine.actions.toolImage(hash).catch(() => undefined),
    );
    const url = () => {
        const loaded = image();
        return loaded ? `data:${loaded.mime};base64,${loaded.data}` : undefined;
    };

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
