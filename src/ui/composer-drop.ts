import { dragHasFiles, dropStagesAttachment, dropTargetActive, nextDragDepth, splitDroppedFiles } from "./drag-drop";
import { createSignal, onCleanup, onMount } from "solid-js";
import { t } from "../state/i18n";

type FileDropOptions = {
    ready: () => boolean;
    addFiles: (files: File[]) => Promise<void>;
    setFileError: (message: string) => void;
};

/** Stages files dropped anywhere in the window; the accessor is true while files are dragged over it. */
export function createWindowFileDrop(options: FileDropOptions) {
    const { ready, addFiles, setFileError } = options;
    const [dropActive, setDropActive] = createSignal(false);

    // Window-level so a drop anywhere attaches; tauri.conf.json disables native drop so WebView2 sends files.
    onMount(() => {
        let depth = 0;

        const update = (transition: Parameters<typeof nextDragDepth>[1]) => {
            depth = nextDragDepth(depth, transition);
            setDropActive(dropTargetActive(depth));
        };

        const onDragEnter = (event: DragEvent) => {
            if (!dragHasFiles(event.dataTransfer?.types)) return;
            event.preventDefault();
            update("enter");
        };

        const onDragOver = (event: DragEvent) => {
            if (!dragHasFiles(event.dataTransfer?.types)) return;
            // preventDefault is required for the drop event to fire at all in WebView2.
            event.preventDefault();
            if (event.dataTransfer) event.dataTransfer.dropEffect = ready() ? "copy" : "none";
        };

        const onDragLeave = (event: DragEvent) => {
            if (!dragHasFiles(event.dataTransfer?.types)) return;
            update("leave");
        };

        const onDragEnd = () => update("end");

        const onDrop = (event: DragEvent) => {
            update("drop");

            if (!dragHasFiles(event.dataTransfer?.types)) return;

            // A missed drop must never make the browser navigate to the dropped file, wherever it landed.
            event.preventDefault();

            if (!ready() || !event.dataTransfer || !dropStagesAttachment(event.target)) return;

            const dropped = splitDroppedFiles(
                Array.from(event.dataTransfer.items ?? []),
                Array.from(event.dataTransfer.files ?? []),
            );
            if (dropped.files.length) void addFiles(dropped.files);
            // After addFiles' synchronous error reset, so the notice survives staging kicking off.
            if (dropped.directories) setFileError(t("drift.composer.folderUnsupported"));
        };

        window.addEventListener("dragenter", onDragEnter);
        window.addEventListener("dragover", onDragOver);
        window.addEventListener("dragleave", onDragLeave);
        window.addEventListener("dragend", onDragEnd);
        window.addEventListener("drop", onDrop);
        onCleanup(() => {
            window.removeEventListener("dragenter", onDragEnter);
            window.removeEventListener("dragover", onDragOver);
            window.removeEventListener("dragleave", onDragLeave);
            window.removeEventListener("dragend", onDragEnd);
            window.removeEventListener("drop", onDrop);
        });
    });

    return dropActive;
}
