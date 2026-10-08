import { composerDraft, patchComposerDraft } from "../state/composer";
import { t } from "../state/i18n";
import {
    formatAttachmentBytes,
    prepareAttachment,
    resolveAttachmentKind,
    unsupportedModelAttachment,
    type AttachmentFailure,
    type AttachmentKind,
} from "../attachments";

import type { ModelInfo } from "../engine/store";

type StagerOptions = {
    scope: () => string;
    selectedModel: () => ModelInfo | undefined;
    setFileError: (message: string) => void;
    /** Called as files start staging; the composer leaves history browsing. */
    staging: () => void;
};

/** Stages files into the scope's draft, removing any the selected model cannot read and reporting why. */
export function createAttachmentStager(options: StagerOptions) {
    const { scope, selectedModel, setFileError } = options;

    async function addFiles(files: Iterable<File>) {
        const key = scope();
        options.staging();
        setFileError("");
        await Promise.all([...files].map((file) => addFile(file, key)));
    }

    async function addFile(file: File, key: string) {
        const resolved = resolveAttachmentKind({ filename: file.name, mime: file.type });
        const id = crypto.randomUUID();
        patchComposerDraft(key, {
            staged: [
                ...composerDraft(key).staged,
                { id, filename: file.name, mime: resolved.mime, size: file.size, status: "processing", meta: {} },
            ],
        });
        const prepared = await prepareAttachment(file, id);
        if (!prepared.ok) {
            patchComposerDraft(key, { staged: composerDraft(key).staged.filter((item) => item.id !== id) });
            showFileFailure(key, file.name, prepared.reason, prepared.kind, prepared.limit);
            return;
        }
        const unsupported = unsupportedModelAttachment(
            [{ filename: prepared.attachment.filename, mime: prepared.attachment.mime }],
            selectedModel(),
        );
        if (unsupported) {
            patchComposerDraft(key, { staged: composerDraft(key).staged.filter((item) => item.id !== id) });
            const selected = selectedModel();
            setFileError(
                t("drift.composer.modelUnsupported", {
                    filename: file.name,
                    kind: t(`drift.attachment.kind.${unsupported.kind}`),
                    model: selected?.name ?? t("command.category.model"),
                }),
            );
            return;
        }
        patchComposerDraft(key, {
            staged: composerDraft(key).staged.map((item) => (item.id === id ? prepared.attachment : item)),
        });
    }

    function showFileFailure(
        key: string,
        filename: string,
        reason: AttachmentFailure,
        kind?: AttachmentKind,
        limit?: number,
    ) {
        if (scope() !== key) return;
        if (reason === "archive" || reason === "binary")
            return setFileError(t("drift.composer.fileUnsupported", { filename }));
        if (reason === "invalid-utf8") return setFileError(t("drift.composer.fileInvalidUtf8", { filename }));
        if (reason === "too-large")
            return setFileError(
                t("drift.composer.fileTooLarge", {
                    filename,
                    kind: t(`drift.attachment.kind.${kind}`),
                    limit: formatAttachmentBytes(limit ?? 0),
                }),
            );
        setFileError(t("drift.composer.fileReadFailed", { filename }));
    }

    return addFiles;
}
