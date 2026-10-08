import { createSignal } from "solid-js";

export type FilePreviewRequest = {
    path: string;
    directory: string;
    line?: number;
    column?: number;
    hash?: string;
};

const [previewFile, setPreviewFile] = createSignal<FilePreviewRequest>();
export { previewFile };

export function openFilePreview(request: FilePreviewRequest) {
    setPreviewFile({ ...request });
}

export function closeFilePreview() {
    setPreviewFile(undefined);
}
