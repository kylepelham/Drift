import { render } from "solid-js/web"
import { runtimeNameFrom } from "./runtime"
import { bootstrapMirror, registerMirrorApplier, startMirrorEvents } from "./state/mirror"
import "./styles/app.css"

const root = document.getElementById("root")!
document.documentElement.dataset.runtime = runtimeNameFrom(window.location)
root.replaceChildren()
root.innerHTML = `<main class="flex h-full items-center justify-center bg-bg text-xs text-ink-muted">Connecting to Drift host...</main>`

void start()

async function start() {
  try {
    await bootstrapMirror()
    const [{ App }, theme, selection, workspaces, navigation] = await Promise.all([
      import("./app"),
      import("./state/theme"),
      import("./state/selection"),
      import("./state/workspaces"),
      import("./state/navigation"),
    ])
    await workspaces.initWorkspaces()
    registerMirrorApplier({
      theme: theme.applyMirroredTheme,
      order: workspaces.applyMirroredWorkspaceOrder,
      selection: (next) => {
        workspaces.applyMirroredWorkspace(next.workspaceId)
        selection.applyMirroredSession(next.sessionId)
        navigation.replaceRemoteSelection(next.workspaceId, next.sessionId)
      },
    })
    startMirrorEvents()
    root.replaceChildren()
    render(() => <App />, root)
  } catch (cause) {
    const message = cause instanceof Error ? cause.message : String(cause)
    root.innerHTML = `<main class="flex h-full items-center justify-center bg-bg p-6 text-ink"><section class="max-w-md text-center"><div class="text-sm font-semibold">Unable to connect to the Drift host</div><p class="mt-2 text-xs leading-relaxed text-ink-muted"></p><button class="mt-4 rounded-md border border-edge px-3 py-2 text-xs">Retry</button></section></main>`
    root.querySelector("p")!.textContent = message
    root.querySelector("button")!.addEventListener("click", () => location.reload())
  }
}
