import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack, type JSX } from "solid-js";
import { alertSounds, customSound, systemNotifications, type AttentionKind } from "../state/prefs";
import { useEngine, type Engine } from "../engine";
import { selectSession } from "../state/selection";
import { playAlertSound } from "./sounds";
import { shellInvoke } from "../shell";
import { t } from "../state/i18n";

// WebView2 stubs the Web Notification API, so the shell path uses the Tauri plugin.
function show(kind: AttentionKind, sessionId: string, title: string, body: string) {
    void playAlertSound(alertSounds()[kind] ?? "none", customSound());

    if (!systemNotifications()[kind] || document.hasFocus()) return;

    const invoke = shellInvoke();
    if (invoke) {
        void invoke("plugin:notification|notify", { options: { title, body } }).catch(() => {});
        return;
    }

    if (typeof Notification === "undefined" || Notification.permission !== "granted") return;

    const notification = new Notification(title, { body, tag: `drift-${sessionId}` });
    notification.onclick = () => {
        window.focus();
        selectSession(sessionId);
        notification.close();
    };
}

export function AttentionNotifier(props: { engine: Engine }) {
    const seen = new Set<string>();
    createEffect(() => {
        const pending = Object.values(props.engine.state.permissions).flat();
        const all = pending;
        const present = new Set(pending.map((permission) => permission.id));
        for (const id of seen) if (!present.has(id)) seen.delete(id);
        for (const permission of all) {
            if (seen.has(permission.id)) continue;
            seen.add(permission.id);
            untrack(() => {
                const session = props.engine.state.sessions[permission.sessionId];
                show(
                    "permission",
                    permission.sessionId,
                    t("notification.permission.title"),
                    `${permission.title} · ${session?.title ?? t("drift.thread.untitled")}`,
                );
            });
        }
    });

    const seenQuestions = new Set<string>();
    createEffect(() => {
        const all = Object.values(props.engine.state.questions).flat();
        const present = new Set(all.map((question) => question.id));
        for (const id of seenQuestions) if (!present.has(id)) seenQuestions.delete(id);
        for (const question of all) {
            if (seenQuestions.has(question.id)) continue;
            seenQuestions.add(question.id);
            untrack(() => {
                const session = props.engine.state.sessions[question.sessionId];
                show(
                    "agent",
                    question.sessionId,
                    t("notification.question.title"),
                    question.questions[0]?.header ?? session?.title ?? t("drift.thread.untitled"),
                );
            });
        }
    });

    const busy = new Set<string>();
    createEffect(() => {
        for (const [sessionId, status] of Object.entries(props.engine.state.status)) {
            const running = status.type === "busy" || status.type === "retry";
            if (running) busy.add(sessionId);
            else if (busy.delete(sessionId) && !props.engine.state.errors[sessionId])
                untrack(() => {
                    const session = props.engine.state.sessions[sessionId];
                    show(
                        "agent",
                        sessionId,
                        t("notification.session.responseReady.title"),
                        session?.title || t("drift.notification.threadFinished"),
                    );
                });
        }
    });

    const errors = new Map<string, string>();
    createEffect(() => {
        for (const [sessionId, error] of Object.entries(props.engine.state.errors)) {
            if (errors.get(sessionId) === error) continue;
            errors.set(sessionId, error);
            untrack(() => {
                const session = props.engine.state.sessions[sessionId];
                show(
                    "error",
                    sessionId,
                    t("notification.session.error.title"),
                    session?.title ? t("drift.notification.threadError", { title: session.title, error }) : error,
                );
            });
        }
        for (const id of errors.keys()) if (!props.engine.state.errors[id]) errors.delete(id);
    });

    return null;
}

export function NoticeHost(props: { children?: JSX.Element }) {
    const engine = useEngine();
    const [now, setNow] = createSignal(Date.now());
    const [dismissed, setDismissed] = createSignal(new Set<string>());
    const timer = setInterval(() => setNow(Date.now()), 250);
    onCleanup(() => clearInterval(timer));

    const visible = createMemo(() =>
        engine.state.notices.filter(
            (notice) => !dismissed().has(notice.id) && notice.created + notice.duration > now(),
        ),
    );

    createEffect(() => {
        const active = new Set(
            engine.state.notices
                .filter((notice) => notice.created + notice.duration > now())
                .map((notice) => notice.id),
        );
        setDismissed((current) => pruneDismissedNoticeIds(current, active));
    });

    const dismiss = (id: string) => setDismissed((current) => new Set([...current, id]));

    return (
        <div class="pointer-events-none fixed top-11 right-5 bottom-5 z-[80] flex w-[min(24rem,calc(100vw-2.5rem))]">
            <div
                class="pointer-events-auto mt-auto flex max-h-full min-h-0 w-full flex-col gap-2 overflow-y-auto overscroll-contain pr-1"
                aria-live="polite"
            >
                <Show when={engine.state.engineError}>
                    <div
                        class="rounded-lg border border-danger/40 bg-surface/95 px-3 py-2 text-danger shadow-xl backdrop-blur"
                        role="alert"
                    >
                        <div class="text-sm font-semibold">{t("drift.engine.stopped.title")}</div>
                        <div class="mt-0.5 text-sm break-words">{engine.state.engineError}</div>
                        <button
                            class="mt-2 rounded-md border border-danger/40 px-2 py-1 text-xs hover:bg-danger/10 disabled:opacity-40"
                            disabled={engine.state.engineRestarting}
                            onClick={() => void engine.restartEngine()}
                        >
                            {engine.state.engineRestarting ? t("drift.engine.restarting") : t("drift.engine.restart")}
                        </button>
                    </div>
                </Show>
                {props.children}
                <For each={visible()}>
                    {(notice) => (
                        <div
                            class="rounded-lg border bg-surface/95 px-3 py-2 shadow-xl backdrop-blur"
                            classList={{
                                "border-edge-strong text-ink": notice.variant === "info",
                                "border-ok/40 text-ok": notice.variant === "success",
                                "border-warn/40 text-warn": notice.variant === "warning",
                                "border-danger/40 text-danger": notice.variant === "error",
                            }}
                            role={notice.variant === "error" ? "alert" : "status"}
                        >
                            <div class="flex items-start gap-3">
                                <div class="min-w-0 flex-1">
                                    <Show when={notice.title}>
                                        {(title) => <div class="text-sm font-semibold">{title()}</div>}
                                    </Show>
                                    <div class="text-sm break-words" classList={{ "mt-0.5": !!notice.title }}>
                                        {notice.message}
                                    </div>
                                </div>
                                <button
                                    class="shrink-0 text-xs opacity-60 hover:opacity-100"
                                    onClick={() => dismiss(notice.id)}
                                >
                                    {t("common.dismiss")}
                                </button>
                            </div>
                        </div>
                    )}
                </For>
            </div>
        </div>
    );
}

export function pruneDismissedNoticeIds(dismissed: Set<string>, active: ReadonlySet<string>): Set<string> {
    const next = new Set([...dismissed].filter((id) => active.has(id)));
    if (next.size === dismissed.size && [...next].every((id) => dismissed.has(id))) return dismissed;
    return next;
}

export function requestNotificationPermission() {
    const invoke = shellInvoke();
    if (invoke) {
        void invoke<boolean>("plugin:notification|is_permission_granted")
            .then((granted) => (granted ? undefined : invoke("plugin:notification|request_permission")))
            .catch(() => {});
        return;
    }

    if (typeof Notification === "undefined") return;

    if (Notification.permission === "default") void Notification.requestPermission();
}
