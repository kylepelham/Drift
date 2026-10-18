import { bootstrapMirror, registerMirrorApplier, startMirrorEvents } from "./state/mirror";
import { loadDictionary, t } from "./state/i18n";
import { language } from "./state/language";
import { runtimeNameFrom } from "./runtime";
import { render } from "solid-js/web";
import "./styles/app.css";

const root = document.getElementById("root")!;
document.documentElement.dataset.runtime = runtimeNameFrom(window.location);
const dictionary = loadDictionary(language());
if (!window.__DRIFT_PRELOAD_READY__) {
    root.replaceChildren();
    // Rendering or the failure screen may already have replaced the placeholder by the time the words arrive.
    void dictionary.then(() => {
        if (root.firstChild) return;
        root.innerHTML = `<main class="flex h-full items-center justify-center bg-bg text-xs text-ink-muted"></main>`;
        root.firstElementChild!.textContent = t("drift.host.connecting");
    });
}

void start();

async function start() {
    try {
        await bootstrapMirror();

        const [{ App }, theme, selection, workspaces, navigation] = await Promise.all([
            import("./app"),
            import("./state/theme"),
            import("./state/selection"),
            import("./state/workspaces"),
            import("./state/navigation"),
        ]);
        await workspaces.initWorkspaces();

        registerMirrorApplier({
            theme: theme.applyMirroredTheme,
            order: workspaces.applyMirroredWorkspaceOrder,
            selection: (next) => {
                workspaces.applyMirroredWorkspace(next.workspaceId);
                selection.applyMirroredSession(next.sessionId);
                navigation.replaceRemoteSelection(next.workspaceId, next.sessionId);
            },
        });
        startMirrorEvents();

        if (document.documentElement.dataset.splash !== "hidden") await window.__DRIFT_PRELOAD_READY__;

        root.replaceChildren();
        render(() => <App />, root);
    } catch (cause) {
        if (document.documentElement.dataset.splash !== "hidden") await window.__DRIFT_PRELOAD_READY__;

        const message = cause instanceof Error ? cause.message : String(cause);
        await dictionary;
        root.innerHTML = `<main class="flex h-full items-center justify-center bg-bg p-6 text-ink"><section class="max-w-md text-center"><div class="text-sm font-semibold"></div><p class="mt-2 text-xs leading-relaxed text-ink-muted"></p><button class="mt-4 rounded-md border border-edge px-3 py-2 text-xs"></button></section></main>`;
        root.querySelector("div")!.textContent = t("drift.host.unreachable");
        root.querySelector("p")!.textContent = message;
        const retry = root.querySelector("button")!;
        retry.textContent = t("drift.preview.retry");
        retry.addEventListener("click", () => location.reload());
    }
}
