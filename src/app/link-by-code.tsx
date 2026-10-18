import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { bindOverlayBack } from "../state/overlay-back";
import { appApi, type Linking } from "./api";
import { t } from "../state/i18n";

const pollMs = 2000;

/**
 * Linking without the camera: the PC shows a code request, the user types the code on the PC, and compares the
 * six words first. Discovery is unauthenticated, so the words are what proves this is their PC.
 */
export function LinkByCode(props: { address: string; port?: number; onCancel: () => void; onLinked: () => void }) {
    const [linking, setLinking] = createSignal<Linking | null>(null);
    const [problem, setProblem] = createSignal("");
    let timer: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;
    onMount(() => onCleanup(bindOverlayBack(props.onCancel)));
    onCleanup(() => {
        disposed = true;
        clearTimeout(timer);
    });

    async function poll(id: string) {
        try {
            const status = await appApi.poll(id);
            if (disposed) return;
            if (status === "linked") return props.onLinked();
            if (status === "expired") return setProblem(t("drift.app.codeExpired"));
        } catch (cause) {
            if (disposed) return;
            setProblem(cause instanceof Error ? cause.message : String(cause));
        }
        timer = setTimeout(() => void poll(id), pollMs);
    }

    onMount(async () => {
        try {
            const started = await appApi.link(props.address, props.port);
            if (disposed) return;
            setLinking(started);
            timer = setTimeout(() => void poll(started.id), pollMs);
        } catch (cause) {
            if (disposed) return;
            setProblem(cause instanceof Error ? cause.message : String(cause));
        }
    });

    return (
        <section class="space-y-4">
            <Show when={problem()}>
                <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger" role="alert">
                    {problem()}
                </div>
            </Show>
            <Show
                when={linking()}
                fallback={
                    <Show when={!problem()}>
                        <p role="status" class="text-sm text-ink-muted">
                            {t("drift.app.connecting")}
                        </p>
                    </Show>
                }
            >
                {(started) => (
                    <div class="space-y-4">
                        <p class="text-sm text-ink-muted">{t("drift.app.typeCode", { name: started().name })}</p>
                        <div class="text-center font-mono text-3xl tracking-widest select-all">{started().code}</div>
                        <div class="space-y-1 rounded-lg border border-edge px-3 py-2">
                            <div class="text-xs text-ink-muted">{t("drift.app.compareWords")}</div>
                            <div class="font-mono text-sm">{started().words.join(" ")}</div>
                        </div>
                        <p class="pulse-soft text-xs text-ink-faint">{t("drift.app.waiting")}</p>
                    </div>
                )}
            </Show>
            <button
                class="h-9 rounded-md border border-edge px-3 text-xs text-ink-muted hover:text-ink"
                onClick={() => props.onCancel()}
            >
                {t("common.cancel")}
            </button>
        </section>
    );
}
