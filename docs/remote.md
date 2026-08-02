# Remote Access

Remote Access serves Drift's complete SolidJS interface to a trusted device on the same LAN. The remote UI uses the same host OpenCode engine, global SSE stream, Drift SQLite data, workspaces, archives, MCP policy, prompts, storage tools, provider state, and voice sidecars as the desktop app.

Remote Access is off by default.

## Enable

1. Open **Settings > Remote Access** in the Drift desktop app.
2. Enable **Remote Access**.
3. Copy the private connection URL and open it on a device connected to the same LAN.

The gateway listens on TCP `41718`. The copied URL has the form:

```text
http://192.168.1.20:41718/companion?token=<access-key>
```

The query key is exchanged once for an HttpOnly, `SameSite=Strict`, `Path=/` cookie. Drift then redirects to the clean `/companion` URL. Use **Rotate access key** to invalidate every existing remote cookie and saved URL immediately. Disabling Remote Access stops both listeners and rejects access immediately; the enabled state and key are retained in Drift's native SQLite store.

If Windows asks whether Drift may accept network connections, allow it on private networks only. A host firewall must permit inbound UDP `41717` and TCP `41718` for discovery and access.

## Deck

Drift is compatible with the existing Flutter Deck app without a Deck update. While Remote Access is enabled, Drift listens for the exact legacy UDP probe on port `41717`:

```text
OPENCODE_COMPANION_DISCOVERY
```

It replies with `kind: "opencode-companion"`, a reachable local IPv4 URL, and the access key expected by Deck. The descriptor also includes `brand: "Drift"`, `name: "Drift"`, `protocol: "drift-remote"`, and `version: 1` for a future dedicated client. Engine credentials are never present in discovery.

## Architecture

```text
mobile browser / Deck WebView
        | authenticated HTTP :41718
        v
Tauri-owned Remote Access gateway
        |-- /companion + /assets/*  embedded Vite dist
        |-- /engine/*               streaming reverse proxy + injected engine Basic auth
        |-- /api/invoke             explicit host-command allowlist
        |-- UDP :41717              Deck discovery while enabled
        v
loopback-only OpenCode engine on a random port
```

`src-tauri/src/remote.rs` owns listener lifecycle, authentication, security headers, static assets, the streaming proxy, RPC dispatch, and discovery. `src/runtime.ts` detects `/companion`; `src/backend.ts` selects Tauri invoke on desktop or same-origin RPC remotely. Host-data modules use that abstraction. Clipboard, links, and notifications remain operations of the current browser or desktop device.

The Vite production output is embedded in the Rust binary with `rust-embed`, including lazy JavaScript chunks, locale chunks, SVG, fonts, and audio. In debug builds the gateway checks the local `dist/` first, so run `bun run build` after frontend changes before testing Remote Access through a native development build.

## Security Model

Remote Access is pragmatic trusted-LAN access, not an Internet-facing service.

- The engine remains on `127.0.0.1` with a random port and random Basic password.
- `/companion`, all assets, `/engine/*`, and `/api/*` require the remote bearer/cookie key.
- API clients may use `Authorization: Bearer <access-key>`.
- Browser `Authorization`, cookies, Origin, Referer, connection headers, and other hop-by-hop headers are stripped before proxying. The gateway injects only the private engine credential.
- RPC uses a fixed command allowlist. Remote Access lifecycle/update/window/shell/plugin commands are not remotely invokable.
- Request bodies are capped, redirects are disabled in the engine proxy, CORS is not enabled, and same-origin requests are expected.
- Host/Origin checks, `no-referrer`, `nosniff`, frame restrictions, and a restrictive Permissions Policy are applied at the gateway.

Anyone who has both LAN access and the private URL can operate the coding agent, read session data, invoke allowed host management functions, and act on host workspaces with the host user's permissions. Do not enable Remote Access on public, guest, hotel, or otherwise untrusted networks. Do not port-forward `41718` or publish it through a tunnel without adding TLS and stronger authentication in front of it.

## HTTP Limitations

LAN traffic is plain HTTP and is not encrypted. The token prevents unauthenticated use but does not protect traffic from a network observer. Complete chat and control work over HTTP. Browser features that require a secure context can vary:

- Android WebView file inputs work and attachments are sent through the engine proxy.
- Clipboard access may require a user gesture or may be unavailable in a generic browser.
- Notifications depend on browser permission and HTTP policy.
- Microphone capture commonly requires HTTPS in generic browsers. If capture is available, host transcription works through RPC; otherwise use text input.
- Opening a code file is an explicit action on the host. Adding a workspace remotely asks for a host filesystem path because a host-native folder dialog is not useful on the remote device.
- Desktop window controls and application updates are hidden remotely and remain host-only.

## Test Procedure

1. Build and verify the code:

   ```bash
   bun install
   bun install --cwd engine/opencode
   bun run typecheck
   bun run test
   bun run build
   cargo test --manifest-path src-tauri/Cargo.toml
   ```

2. Build the required sidecars if they are not already present, then run Drift natively.
3. Enable **Settings > Remote Access** and copy the private URL.
4. From another LAN device, open the copied URL. Confirm the browser redirects to clean `/companion` and the full workspace/session UI hydrates.
5. Start a prompt on one device and confirm transcript/tool/SSE updates appear on both. Exercise permission and question replies, attachments, undo/redo, model selection, settings, storage, prompts, and MCP management.
6. Put the Deck app on the same LAN and confirm it discovers **Drift** and opens the same interface.
7. Rotate the key and confirm the old browser is rejected on its next request. Reconnect with the newly copied URL.
8. Disable Remote Access and confirm HTTP access and Deck discovery stop immediately.

Physical-device checks should cover Android Back behavior, display cutouts/safe areas, the software keyboard while composing, file selection, coarse-pointer menus, sleep/resume SSE recovery, and a `1280x800` Deck viewport.
