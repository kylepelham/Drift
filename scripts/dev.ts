const port = process.env.DRIFT_ENGINE_PORT ?? "4096"
const engine = Bun.spawn(["cmd", "/c", "opencode", "serve", "--port", port], {
  stdout: "inherit",
  stderr: "inherit",
})
const vite = Bun.spawn(["bun", "x", "vite"], {
  stdout: "inherit",
  stderr: "inherit",
  env: {
    ...process.env,
    VITE_ENGINE_URL: `http://127.0.0.1:${port}`,
    VITE_ENGINE_USERNAME: process.env.OPENCODE_SERVER_USERNAME ?? "opencode",
    VITE_ENGINE_PASSWORD: process.env.OPENCODE_SERVER_PASSWORD ?? "",
  },
})

function shutdown() {
  engine.kill()
  vite.kill()
  process.exit(0)
}
process.on("SIGINT", shutdown)
process.on("SIGTERM", shutdown)
await Promise.race([engine.exited, vite.exited])
shutdown()
