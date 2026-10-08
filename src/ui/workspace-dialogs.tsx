import { createSignal, For, onCleanup, onMount, Show, untrack } from "solid-js";
import { removeWorkspace, updateWorkspace } from "../state/workspaces";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { workspaceInitials } from "./workspace-presentation";
import { createDismissOnOutside } from "./dismiss";
import { fixedMenuPosition } from "../state/zoom";
import { normalizeDir } from "../engine/store";
import { t } from "../state/i18n";

import type { Workspace } from "../state/store";

export type WorkspaceMenuState = { x: number; y: number; workspaceId: string };
export type SessionMenuState = { x: number; y: number; sessionId: string; workspaceId: string };

function MenuItem(props: { label: string; danger?: boolean; disabled?: boolean; onClick: () => void }) {
    return (
        <button
            class="w-full rounded-md px-2 py-1.5 text-left text-sm transition-colors"
            classList={{
                "text-ink-muted hover:bg-raised hover:text-ink": !props.danger && !props.disabled,
                "text-danger hover:bg-danger/10": props.danger && !props.disabled,
                "cursor-default text-ink-faint": props.disabled,
            }}
            disabled={props.disabled}
            onClick={() => props.onClick()}
        >
            {props.label}
        </button>
    );
}

export function WorkspaceMenu(props: {
    state: WorkspaceMenuState;
    workspace: Workspace;
    onEdit: () => void;
    onMove: () => void;
    onClose: () => void;
}) {
    let root!: HTMLDivElement;
    const [confirming, setConfirming] = createSignal(false);
    const position = () => fixedMenuPosition(props.state.x, props.state.y, 212, confirming() ? 176 : 144);

    createDismissOnOutside({ inside: () => [root], onDismiss: () => props.onClose(), escape: true });

    return (
        <div
            ref={root}
            class="fade-up fixed z-40 w-52 rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
            style={{ left: `${position().left}px`, top: `${position().top}px` }}
        >
            <MenuItem
                label={t("common.edit")}
                onClick={() => {
                    props.onEdit();
                    props.onClose();
                }}
            />
            <MenuItem
                label={t("drift.workspace.move")}
                onClick={() => {
                    props.onMove();
                    props.onClose();
                }}
            />
            <MenuItem
                label={confirming() ? t("drift.workspace.confirmRemove") : t("drift.workspace.remove")}
                danger
                onClick={() => {
                    if (!confirming()) return setConfirming(true);

                    void removeWorkspace(props.workspace.id);
                    props.onClose();
                }}
            />
            <Show when={confirming()}>
                <div class="px-2 pt-1 pb-0.5 text-[0.65rem] leading-snug text-ink-faint">
                    {t("drift.workspace.removeHint")}
                </div>
            </Show>
        </div>
    );
}

export function SessionMenu(props: {
    state: SessionMenuState;
    workspaces: Workspace[];
    onMove: (workspace: Workspace) => void;
    onClose: () => void;
}) {
    let root!: HTMLDivElement;
    const [choosing, setChoosing] = createSignal(false);
    const targets = () => {
        const source = props.workspaces.find((workspace) => workspace.id === props.state.workspaceId);

        return props.workspaces.filter(
            (workspace) =>
                workspace.id !== props.state.workspaceId &&
                (!source || normalizeDir(workspace.path) !== normalizeDir(source.path)),
        );
    };
    const height = () => (choosing() ? Math.min(320, 48 + Math.max(1, targets().length) * 36) : 48);
    const position = () => fixedMenuPosition(props.state.x, props.state.y, 212, height());

    createDismissOnOutside({ inside: () => [root], onDismiss: () => props.onClose(), escape: true });

    return (
        <div
            ref={root}
            class="fade-up fixed z-40 max-h-80 w-52 overflow-y-auto rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
            style={{ left: `${position().left}px`, top: `${position().top}px` }}
        >
            <MenuItem
                label={choosing() ? t("drift.thread.moveToWorkspace") : t("drift.thread.move")}
                onClick={() => setChoosing(true)}
            />
            <Show when={choosing()}>
                <div class="my-1 border-t border-edge" />
                <For each={targets()}>
                    {(workspace) => (
                        <MenuItem
                            label={workspace.name}
                            onClick={() => {
                                props.onMove(workspace);
                                props.onClose();
                            }}
                        />
                    )}
                </For>
                <Show when={targets().length === 0}>
                    <MenuItem label={t("drift.thread.noOtherWorkspaces")} disabled onClick={() => {}} />
                </Show>
            </Show>
        </div>
    );
}

export function WorkspaceEditModal(props: { workspace: Workspace; onClose: () => void }) {
    let dialog!: HTMLDivElement;
    const [name, setName] = createSignal(untrack(() => props.workspace.name));
    const [icon, setIcon] = createSignal(untrack(() => props.workspace.icon));

    onMount(() => onCleanup(activateModal(dialog, props.onClose)));

    async function save() {
        const next = name().trim();
        await updateWorkspace(props.workspace.id, { name: next || props.workspace.name, icon: icon() });

        props.onClose();
    }

    return (
        <div
            data-modal-layer
            class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
            onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
        >
            <div
                ref={dialog}
                role="dialog"
                aria-modal="true"
                aria-label={t("dialog.project.edit.title")}
                tabIndex={-1}
                class="fade-up w-96 rounded-xl border border-edge bg-overlay p-4 shadow-2xl shadow-black/40"
                onClick={(event) => event.stopPropagation()}
            >
                <div class="mb-4 text-sm font-semibold text-ink">{t("dialog.project.edit.title")}</div>
                <div class="mb-4 flex items-center gap-3">
                    <Show
                        when={icon().startsWith("data:")}
                        fallback={
                            <span class="flex size-12 items-center justify-center rounded-lg bg-raised text-sm font-semibold text-ink-muted">
                                {workspaceInitials(name() || props.workspace.name)}
                            </span>
                        }
                    >
                        <img src={icon()} alt="" class="size-12 rounded-lg object-cover" />
                    </Show>
                    <div class="flex flex-col gap-1.5">
                        <button
                            class="rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                            onClick={() => void pickIconImage().then((image) => image && setIcon(image))}
                        >
                            {t("drift.workspace.changeImage")}
                        </button>
                        <Show when={icon()}>
                            <button
                                class="rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                                onClick={() => setIcon("")}
                            >
                                {t("drift.workspace.useInitials")}
                            </button>
                        </Show>
                    </div>
                </div>
                <label class="mb-4 block">
                    <span class="mb-1 block text-[0.68rem] tracking-wide text-ink-faint uppercase">
                        {t("dialog.project.edit.name")}
                    </span>
                    <input
                        class="w-full rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm outline-none focus:border-edge-strong"
                        value={name()}
                        onInput={(event) => setName(event.currentTarget.value)}
                        onKeyDown={(event) => event.key === "Enter" && void save()}
                    />
                </label>
                <div class="mb-3 text-[0.68rem] text-ink-faint">{props.workspace.path}</div>
                <div class="flex justify-end gap-2">
                    <button
                        class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:text-ink"
                        onClick={() => props.onClose()}
                    >
                        {t("common.cancel")}
                    </button>
                    <button
                        class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink"
                        onClick={() => void save()}
                    >
                        {t("common.save")}
                    </button>
                </div>
            </div>
        </div>
    );
}

async function pickIconImage(): Promise<string | null> {
    const file = await pickFile("image/*");
    if (!file) return null;

    const bitmap = await createImageBitmap(file);
    const size = 64;
    const canvas = document.createElement("canvas");
    canvas.width = size;
    canvas.height = size;

    const context = canvas.getContext("2d")!;
    const scale = Math.max(size / bitmap.width, size / bitmap.height);
    const width = bitmap.width * scale;
    const height = bitmap.height * scale;
    context.drawImage(bitmap, (size - width) / 2, (size - height) / 2, width, height);
    bitmap.close();

    return canvas.toDataURL("image/webp", 0.85);
}

function pickFile(accept: string): Promise<File | null> {
    return new Promise((resolve) => {
        const input = document.createElement("input");
        input.type = "file";
        input.accept = accept;
        input.onchange = () => resolve(input.files?.[0] ?? null);
        input.oncancel = () => resolve(null);

        input.click();
    });
}
