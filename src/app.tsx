import { EngineProvider } from "./engine"
import { bindTheme } from "./state/theme"
import { AttentionStrip } from "./ui/attention"
import { Chat } from "./ui/chat"
import { Composer } from "./ui/composer"
import { Sidebar } from "./ui/sidebar"
import { Titlebar } from "./ui/titlebar"

export function App() {
  bindTheme()
  return (
    <EngineProvider>
      <div class="flex h-full flex-col bg-bg text-ink">
        <Titlebar />
        <div class="flex min-h-0 flex-1">
          <Sidebar />
          <main class="flex min-w-0 flex-1 flex-col">
            <Chat />
            <AttentionStrip />
            <Composer />
          </main>
        </div>
      </div>
    </EngineProvider>
  )
}
