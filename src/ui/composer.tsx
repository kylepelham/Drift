import { clearEdits, modelVisible, prefsFor, seedPrefs, sendableVariant, updatePrefs } from "../state/prefs";
import { createEffect, createMemo, createSignal, For, onMount, Show, untrack } from "solid-js";
import { createComposerSubmissionGuard, createComposerSubmit } from "./composer-submit";
import { prepareAttachmentsForSend, unsupportedModelAttachment } from "../attachments";
import { dictationActive, stopDictation, toggleDictation } from "../voice/dictation";
import { modelInfo, resolveModel, savedChoice, sessionBusy } from "../engine/store";
import { createMentionAutocomplete, mentionFiles } from "./composer-mentions";
import { emitThreadCreated, transformComposerSubmit } from "../plugins";
import { defaultVisibleModelIds, ModelManager } from "./model-manager";
import { modelInstalled, refreshVoiceModels } from "../voice/models";
import { selectedSession, selectSession } from "../state/selection";
import { agentLabel, reasoningLevelLabel, t } from "../state/i18n";
import { interruptResponseAnimations } from "./response-animation";
import { IconMic, IconPaperclip, IconShieldCheck } from "./icons";
import { dictationEnabled, dictationModel } from "../state/voice";
import { createAttachmentStager } from "./composer-attachments";
import { ComposerMentionMenu } from "./composer-mention-menu";
import { AttachmentChip } from "./composer-attachment-chip";
import { ComposerSlashMenu } from "./composer-slash-menu";
import { ComposerAttention } from "./composer-attention";
import { connectedModelItems } from "./composer-models";
import { createWindowFileDrop } from "./composer-drop";
import { appendDictation } from "../voice/transcript";
import { activeWorkspace } from "../state/workspaces";
import { createSlashMenu } from "./composer-slash";
import { Picker, type PickerItem } from "./picker";
import { DictationStatus } from "./composer-voice";
import { variantNames } from "../engine/catalog";
import { ProviderIcon } from "./provider-icon";
import { onKeybind } from "../state/keybinds";
import { openSettings } from "./settings";
import { shellInvoke } from "../shell";
import { useEngine } from "../engine";
import {
    canNavigateComposerHistory,
    clearComposerDraft,
    composerDraft,
    composerHistory,
    composerScope,
    migrateComposerDraft,
    navigateComposerHistory,
    patchComposerDraft,
    recordComposerHistory,
    setComposerDraft,
    type ComposerDraft,
    type StagedFile,
} from "../state/composer";

// Autosize ceiling for the textarea. Must stay in sync with the `max-h-50` class on the textarea
// (Tailwind spacing 50 = 12.5rem = 200px); otherwise the element and its inline height disagree.
const maxComposerHeightPx = 200;
// The OS clipboard is written after the browser finishes its own copy, so ours lands last and wins.
const clipboardRepublishDelayMs = 100;

export function composerSelection(value: string, start: number, end: number) {
    return value.slice(Math.min(start, end), Math.max(start, end));
}

export function Composer() {
    const engine = useEngine();
    const [manageModels, setManageModels] = createSignal(false);
    const [fileError, setFileError] = createSignal("");
    const [submissionVersion, setSubmissionVersion] = createSignal(0);
    const [historyNavigation, setHistoryNavigation] = createSignal<{
        scope: string;
        index: number;
        saved: ComposerDraft | null;
        displayed: ComposerDraft;
    } | null>(null);
    let area!: HTMLTextAreaElement;
    let areaFrame!: HTMLDivElement;
    let filePicker!: HTMLInputElement;

    const submissionGuard = createComposerSubmissionGuard(() => setSubmissionVersion((value) => value + 1));

    const scope = () => composerScope(selectedSession(), activeWorkspace()?.id);
    const draft = () => composerDraft(scope()).text;
    const staged = () => composerDraft(scope()).staged;
    const mentions = () => composerDraft(scope()).mentions;
    const setDraft = (text: string) => {
        setHistoryNavigation(null);
        patchComposerDraft(scope(), { text });
    };
    const setStaged = (value: StagedFile[] | ((current: StagedFile[]) => StagedFile[])) => {
        setHistoryNavigation(null);
        const key = scope();
        const current = composerDraft(key).staged;
        patchComposerDraft(key, { staged: typeof value === "function" ? value(current) : value });
    };
    const setMentions = (mentions: string[]) => patchComposerDraft(scope(), { mentions });

    // Only finalized speech reaches the draft, so live text can never rewrite what was typed.
    function appendVoice(segment: string) {
        const key = scope();
        setHistoryNavigation(null);
        patchComposerDraft(key, { text: appendDictation(composerDraft(key).text, segment) });
    }

    function toggleVoice() {
        if (!modelInstalled(dictationModel())) return openSettings("Voice");
        void toggleDictation(appendVoice);
    }

    const addFiles = createAttachmentStager({
        scope,
        selectedModel: () => modelInfo(engine.state, resolveModel(engine.state, prefs().model)),
        setFileError,
        staging: () => setHistoryNavigation(null),
    });

    const dropActive = createWindowFileDrop({ ready: () => ready(), addFiles, setFileError });

    let previousScope = scope();
    createEffect(() => {
        const nextScope = scope();
        if (nextScope !== previousScope) {
            const navigation = untrack(historyNavigation);
            if (navigation?.saved && composerDraft(navigation.scope) === navigation.displayed) {
                setComposerDraft(navigation.scope, navigation.saved);
            }
            setHistoryNavigation(null);
            previousScope = nextScope;
        }
        slash.setDismissed(false);
        mention.setQuery(null);
        setFileError("");
    });

    // Tracks draft, not just scope: programmatic restores (revert, /undo) must re-measure.
    createEffect(() => {
        draft();
        queueMicrotask(() => {
            if (!area) return;
            resize();
        });
    });

    const slash = createSlashMenu({
        engine,
        area: () => area,
        draft,
        setDraft,
        resize: () => resize(),
    });

    const mention = createMentionAutocomplete({
        area: () => area,
        draft,
        setDraft,
        mentions,
        setMentions,
        ready: () => ready(),
        findFiles: (query) => engine.actions.findFiles(query),
        resize: () => resize(),
    });

    const busy = () => {
        const id = selectedSession();
        return !!id && sessionBusy(engine.state, id);
    };
    const online = () => engine.state.connection === "online";
    const ready = () => online() && !!activeWorkspace();
    const placeholder = () => {
        if (!activeWorkspace()) return t("drift.composer.selectWorkspace");
        if (!online()) return t("drift.composer.connecting");
        return busy() ? `${t("drift.prompt.steer")}...` : t("prompt.placeholder.simple");
    };

    const availableModelItems = createMemo<PickerItem[]>(() => connectedModelItems(engine.state));
    const defaultModelIds = createMemo(() => defaultVisibleModelIds(availableModelItems()));
    const modelItems = createMemo(() =>
        availableModelItems().filter((item) => modelVisible(item.id, defaultModelIds().has(item.id))),
    );

    const agentItems = createMemo<PickerItem[]>(() =>
        engine.state.agents
            .filter((agent) => agent.mode !== "subagent" && !agent.hidden)
            .map((agent) => ({ id: agent.name, label: agentLabel(agent.name), hint: agent.description })),
    );

    const prefs = () => prefsFor(selectedSession(), savedChoice(engine.state, selectedSession()));
    const model = () => resolveModel(engine.state, prefs().model);
    const modelName = () => modelInfo(engine.state, model())?.name;
    const modelId = () => {
        const ref = model();
        return ref ? `${ref.providerID}/${ref.modelID}` : undefined;
    };

    const variants = createMemo(() => variantNames(modelInfo(engine.state, model())));
    const variantItems = createMemo<PickerItem[]>(() => [
        { id: "default", label: t("common.default") },
        ...variants().map((name) => ({ id: name, label: reasoningLevelLabel(name) })),
    ]);
    const variant = () => {
        const pref = prefs().variant;
        return pref && variants().includes(pref) ? pref : undefined;
    };

    const submit = createComposerSubmit(
        {
            scope,
            session: selectedSession,
            workspace: activeWorkspace,
            online,
            draft: composerDraft,
            prepare(existing) {
                const selectedPrefs = prefsFor(existing, savedChoice(engine.state, existing));
                const selectedModel = resolveModel(engine.state, selectedPrefs.model);
                const selectedVariants = variantNames(modelInfo(engine.state, selectedModel));
                return {
                    selectedPrefs,
                    selectedModel,
                    selectedVariant: sendableVariant(selectedPrefs.variant, selectedVariants),
                };
            },
            transform: transformComposerSubmit,
            newSession: engine.actions.newSession,
            sessionScope: (id) => composerScope(id),
            migrateDraft: migrateComposerDraft,
            selectSession,
            sessionCreated(id) {
                seedPrefs(id);
                emitThreadCreated(id);
            },
            async send(id, text, snapshot, workspace, prepared) {
                const unsupported = unsupportedModelAttachment(
                    snapshot.staged,
                    modelInfo(engine.state, prepared.selectedModel),
                );
                if (unsupported) {
                    const message = t("drift.composer.modelUnsupported", {
                        filename: unsupported.attachment.filename,
                        kind: t(`drift.attachment.kind.${unsupported.kind}`),
                        model: modelInfo(engine.state, prepared.selectedModel)?.name ?? t("command.category.model"),
                    });
                    setFileError(message);
                    return { ok: false as const, error: message };
                }
                const attachments = await prepareAttachmentsForSend(snapshot.staged);
                const files = [...mentionFiles(text, snapshot.mentions, workspace.path), ...attachments.files];
                const prompt = [text, attachments.text].filter(Boolean).join("\n\n");
                const result = await engine.actions.send(id, prompt, {
                    model: prepared.selectedModel,
                    agent: prepared.selectedPrefs.agent,
                    variant: prepared.selectedVariant,
                    files,
                });
                if (result.ok) clearEdits(id);
                return result;
            },
            admitted(key, snapshot, historyDraft) {
                stopDictation();
                recordComposerHistory(historyDraft);
                setHistoryNavigation(null);
                clearComposerDraft(key, snapshot);
                setFileError("");
                resize();
                queueMicrotask(() => area.focus());
            },
            failed(error) {
                engine.actions.notice({
                    message: error instanceof Error ? error.message : String(error),
                    variant: "error",
                });
            },
        },
        submissionGuard,
    );

    const submitting = () => {
        submissionVersion();
        return submissionGuard.has(scope());
    };

    function onKey(event: KeyboardEvent) {
        if (event.isComposing) return;
        if (event.key === "Enter" && !event.shiftKey && submitting()) {
            event.preventDefault();
            return;
        }
        if (mention.open() && mention.handleKey(event)) return;
        if (slash.active() && slash.handleKey(event)) return;
        if (["ArrowUp", "ArrowDown"].includes(event.key) && browseHistory(event)) return;
        if (event.key === "Tab") {
            event.preventDefault();
            cycleAgent(event.shiftKey ? -1 : 1);
            return;
        }
        if (event.key !== "Enter" || event.shiftKey) return;
        event.preventDefault();
        void submit();
    }

    function browseHistory(event: KeyboardEvent) {
        if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false;
        if (area.selectionStart !== area.selectionEnd) return false;
        const direction = event.key === "ArrowUp" ? "up" : "down";
        const active = historyNavigation();
        if (!canNavigateComposerHistory(direction, draft(), area.selectionStart, !!active)) return false;
        const result = navigateComposerHistory(
            composerHistory(),
            { index: active?.index ?? -1, saved: active?.saved ?? null },
            composerDraft(scope()),
            direction,
        );
        if (!result) return false;
        const key = scope();
        setComposerDraft(key, result.draft);
        setHistoryNavigation(
            result.navigation.index < 0
                ? null
                : {
                      scope: key,
                      index: result.navigation.index,
                      saved: result.navigation.saved,
                      displayed: result.draft,
                  },
        );
        slash.setDismissed(true);
        mention.setQuery(null);
        event.preventDefault();
        queueMicrotask(() => {
            resize();
            area.focus();
            const position = result.cursor === "start" ? 0 : result.draft.text.length;
            area.setSelectionRange(position, position);
        });
        return true;
    }

    function cycleAgent(step: number) {
        const items = agentItems();
        if (items.length < 2) return;
        const index = items.findIndex((item) => item.id === prefs().agent);
        updatePrefs(selectedSession(), { agent: items[(index + step + items.length) % items.length].id });
    }

    function resize() {
        // Keep the composer's outer height stable while the live textarea is temporarily `auto` for
        // measurement. Otherwise every key collapses a capped draft to one row, lets the transcript
        // viewport grow and clamp its scroll position, then snaps it back after the height is restored.
        const current = area.offsetHeight;
        areaFrame.style.height = `${current}px`;
        area.style.height = "auto";
        const scrollHeight = area.scrollHeight;
        const next = Math.min(scrollHeight, maxComposerHeightPx);
        area.style.height = `${next}px`;
        area.style.overflowY = scrollHeight > maxComposerHeightPx ? "auto" : "hidden";
        if (next !== current) areaFrame.style.height = `${next}px`;
    }

    function republishComposerSelection(event: ClipboardEvent & { currentTarget: HTMLTextAreaElement }) {
        const target = event.currentTarget;
        const text = composerSelection(target.value, target.selectionStart, target.selectionEnd);
        const invoke = shellInvoke();
        if (!text || !invoke) return;
        setTimeout(
            () => void invoke("clipboard_write_text", { text }).catch(() => undefined),
            clipboardRepublishDelayMs,
        );
    }

    // The engine answers auto-accepted asks itself, with no window open; this only shows and switches it.
    const sessionAutoAccept = () => !!engine.state.sessions[selectedSession() ?? ""]?.autoAccept;
    const autoAcceptOn = () => engine.state.autoAcceptAll || sessionAutoAccept();
    const autoAcceptHint = () =>
        t(engine.state.autoAcceptAll ? "drift.permissions.autoGlobal" : "drift.permissions.autoThread");
    const toggleAutoAccept = () => {
        const id = selectedSession();
        if (id && !engine.state.autoAcceptAll) void engine.actions.setAutoAccept(id, !sessionAutoAccept());
    };

    onMount(() => {
        if (dictationEnabled()) void refreshVoiceModels();
        return onKeybind("autoAccept", toggleAutoAccept);
    });

    const sendDisabled = () =>
        (!draft().trim() && staged().length === 0) ||
        staged().some((file) => file.status === "processing") ||
        !ready() ||
        submitting();

    return (
        <div class="composer-shell relative z-10">
            <ComposerAttention />
            <div class="relative mx-auto max-w-3xl rounded-xl border border-edge bg-surface transition-colors focus-within:border-edge-strong">
                <Show when={dropActive() && ready()}>
                    <div class="pointer-events-none absolute inset-0 z-30 flex items-center justify-center rounded-xl border-2 border-dashed border-accent bg-surface/85">
                        <span class="text-sm font-medium text-accent">{t("drift.composer.dropFiles")}</span>
                    </div>
                </Show>
                <Show when={mention.open()}>
                    <ComposerMentionMenu mention={mention} />
                </Show>
                <Show when={slash.open()}>
                    <ComposerSlashMenu menu={slash} />
                </Show>
                <Show when={staged().length > 0 || fileError()}>
                    <div class="flex flex-wrap items-center gap-2 px-3 pt-2.5">
                        <For each={staged()}>
                            {(file) => {
                                const remove = () => setStaged(staged().filter((item) => item.id !== file.id));
                                return <AttachmentChip file={file} remove={remove} />;
                            }}
                        </For>
                        <Show when={fileError()}>
                            <span class="text-xs text-danger">{fileError()}</span>
                        </Show>
                    </div>
                </Show>
                <DictationStatus />
                <div ref={areaFrame} class="w-full">
                    <textarea
                        ref={area}
                        role={slash.active() ? "combobox" : undefined}
                        aria-expanded={slash.open()}
                        aria-autocomplete="list"
                        aria-controls={slash.open() ? slash.id : undefined}
                        aria-activedescendant={slash.activeOptionId()}
                        rows={1}
                        class="max-h-50 w-full resize-none overflow-y-hidden bg-transparent px-4 pt-3 pb-1 text-[0.925rem] outline-none placeholder:text-ink-faint"
                        placeholder={placeholder()}
                        disabled={!ready()}
                        value={draft()}
                        onInput={(event) => {
                            setDraft(event.currentTarget.value);
                            slash.setDismissed(false);
                            slash.setCursor(0);
                            mention.refresh();
                        }}
                        onClick={() => mention.refresh()}
                        onCopy={republishComposerSelection}
                        onCut={republishComposerSelection}
                        onPaste={(event) => {
                            if (!event.clipboardData?.files.length) return;
                            event.preventDefault();
                            void addFiles(event.clipboardData.files);
                        }}
                        onKeyDown={onKey}
                    />
                </div>
                <div class="composer-actions flex min-w-0 items-center gap-1 px-2.5 pb-2">
                    <div class="composer-options relative flex min-w-0 flex-1 items-center gap-1">
                        <Picker
                            label={t("command.category.agent")}
                            items={agentItems()}
                            selected={prefs().agent}
                            fallbackLabel={agentLabel(prefs().agent)}
                            onPick={(id) => updatePrefs(selectedSession(), { agent: id })}
                        />
                        <Picker
                            label={t("command.category.model")}
                            items={modelItems()}
                            selected={modelId()}
                            icon={<ProviderIcon id={model()?.providerID} class="size-3.5 shrink-0" />}
                            fallbackLabel={modelName()}
                            onManage={() => setManageModels(true)}
                            onPick={(id) => {
                                const [providerID, ...rest] = id.split("/");
                                updatePrefs(selectedSession(), { model: { providerID, modelID: rest.join("/") } });
                            }}
                        />
                        <Show when={variants().length > 0}>
                            <Picker
                                label={t("drift.composer.thinkingLevel")}
                                items={variantItems()}
                                selected={variant() ?? "default"}
                                onPick={(id) =>
                                    updatePrefs(selectedSession(), { variant: id === "default" ? null : id })
                                }
                            />
                        </Show>
                    </div>
                    <div class="composer-action-buttons ml-auto flex shrink-0 items-center gap-1">
                        <Show when={autoAcceptOn()}>
                            <button
                                class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink disabled:cursor-default disabled:opacity-60"
                                title={autoAcceptHint()}
                                aria-label={t("command.permissions.autoaccept.disable")}
                                disabled={engine.state.autoAcceptAll}
                                onClick={toggleAutoAccept}
                            >
                                <IconShieldCheck class="size-3.5" />
                            </button>
                        </Show>
                        <input
                            ref={filePicker}
                            type="file"
                            multiple
                            class="hidden"
                            onChange={(event) => {
                                if (event.currentTarget.files) void addFiles(event.currentTarget.files);
                                event.currentTarget.value = "";
                            }}
                        />
                        <Show when={dictationEnabled()}>
                            <button
                                title={dictationActive() ? t("drift.voice.stop") : t("drift.voice.start")}
                                aria-label={dictationActive() ? t("drift.voice.stop") : t("drift.voice.start")}
                                aria-pressed={dictationActive()}
                                class="flex size-7 items-center justify-center rounded-md transition-colors hover:bg-raised disabled:cursor-default disabled:opacity-60"
                                classList={{
                                    "text-ink-faint hover:text-ink": !dictationActive(),
                                    "text-danger": dictationActive(),
                                }}
                                disabled={!ready()}
                                onClick={toggleVoice}
                            >
                                <IconMic class="size-4" />
                            </button>
                        </Show>
                        <button
                            title={t("prompt.action.attachFile")}
                            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                            disabled={!ready()}
                            onClick={() => filePicker.click()}
                        >
                            <IconPaperclip class="size-4" />
                        </button>
                        <Show when={busy()}>
                            <button
                                class="rounded-md border border-edge px-3 py-1 text-xs text-ink-muted transition-colors hover:border-danger hover:text-danger"
                                title={t("prompt.action.stop")}
                                onClick={() => {
                                    interruptResponseAnimations();
                                    void engine.actions.abort(selectedSession()!);
                                }}
                            >
                                {t("prompt.action.stop")}
                            </button>
                        </Show>
                        <button
                            class="composer-submit rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink transition-opacity disabled:opacity-40"
                            title={busy() ? t("drift.prompt.steer") : t("prompt.action.send")}
                            disabled={sendDisabled()}
                            onClick={() => void submit()}
                        >
                            {busy() ? t("drift.prompt.steer") : t("prompt.action.send")}
                        </button>
                    </div>
                </div>
            </div>
            <Show when={manageModels()}>
                <ModelManager items={availableModelItems()} onClose={() => setManageModels(false)} />
            </Show>
        </div>
    );
}
