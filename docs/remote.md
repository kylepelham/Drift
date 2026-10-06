# Remote Access

Remote Access serves Drift's complete SolidJS interface to your phone, tablet, or another computer on the same network. The remote UI uses the same Drift engine, event stream, Drift SQLite data, workspaces, archives, MCP policy, prompts, storage tools, provider state, and voice transcription as the desktop app.

Remote Access is off by default. Traffic is always encrypted with HTTPS, and only devices you approve can connect.

## Connect a device

1. In the desktop app, open **Settings > Remote Access** and turn on **Enable Remote Access**.
2. On the device, scan the QR code under **Connect a device** with its camera, or open the address shown next to it. The address has the form `https://192.168.1.20:41718/companion` and holds no secret, so it is safe to share. Typing it without `https://` also works: plain HTTP on the same port redirects to HTTPS.
3. The first visit shows a browser certificate warning (see [Encryption](#encryption)). Continue, or install the certificate first.
4. The device shows an 8-character code such as `K7QD-M3PX`. On the desktop, type it in step 3 of **Connect a device** and press **Link**. A desktop notice also appears while a device is waiting, with a shortcut that opens the page with the field focused. The device opens Drift within two seconds.

Codes expire after 10 minutes and approve exactly one device. The code is only ever typed into the desktop, never into the gateway, so it cannot be guessed over the network. Each address may hold three waiting codes, and the gateway holds at most 32.

### Password sign-in (optional)

Under **Password sign-in**, choose **Set up** and enter a username and password (8 to 256 characters). The device's sign-in page then also offers a username and password form, which is useful when you are away from the desktop. Changing or turning off the password signs out every device that signed in with it; devices linked with a code stay signed in.

Passwords are stored as PBKDF2-HMAC-SHA256 with a random 16-byte salt and 600,000 iterations, in Drift's SQLite `app_setting` table. After five wrong attempts from one address, sign-in from that address locks for 30 seconds, doubling per further failure up to 15 minutes. At most two password checks run at once.

### Linked devices

**Devices** lists every signed-in device with how it signed in and when it was last active. **Sign out** revokes one device; **Sign out all devices** revokes all of them. Revocation closes open streams and the engine event socket immediately, and the device returns to the sign-in page on its next request. A remote device can also sign itself out from its own **Settings > Remote Access**.

Each device holds its own random 256-bit session token in an `HttpOnly; Secure; SameSite=Strict` cookie that lasts 400 days. Drift stores only the token's SHA-256 in the `remote_device` table, so the database never contains a usable credential. API clients may send the same token as `Authorization: Bearer <token>`.

## Encryption

The gateway only speaks TLS (1.2 or 1.3, HTTP/2 or HTTP/1.1) on TCP `41718`. On first use Drift creates a private certificate authority for this computer and keeps it in the app data directory (`remote-tls/`). For each local address a device connects to, Drift issues a short-lived leaf certificate (397 days, regenerated on restart) signed by that authority, so changing Wi-Fi networks or DHCP leases needs no manual steps.

Browsers do not know this authority, so they warn on first connect. You can:

- **Continue past the warning.** Traffic is still encrypted. The weakness is the first visit: you cannot tell Drift's certificate from an impostor's unless you compare fingerprints (below).
- **Install the certificate** from the **Download the certificate** link on the device's sign-in page (`/auth/certificate`). Afterwards there is no warning, and any impostor certificate would trigger one.
  - Android: Settings > Security > Encryption & credentials > Install a certificate > CA certificate.
  - iPhone and iPad: install the downloaded profile, then enable it in Settings > General > About > Certificate Trust Settings.
  - Windows, macOS, Linux: import it as a trusted root certificate authority.

Before installing, check that the certificate's SHA-256 fingerprint matches the one under **Stop the certificate warning** in the desktop settings. The authority carries X.509 name constraints that only permit private addresses (10/8, 172.16/12, 192.168/16, 100.64/10 including Tailscale, 169.254/16, 127/8, `localhost`, and `.local`), so trusting it cannot be abused to impersonate public websites even if its key were stolen.

A trusted certificate also makes the page a secure context, which browsers require for features such as microphone capture and the clipboard.

If Windows asks whether Drift may accept network connections, allow it on private networks only. A host firewall must permit inbound UDP `41717` for address discovery and TCP `41718` for access.

## Discovery

While Remote Access is enabled, Drift listens for the existing UDP probe on port `41717`:

```text
OPENCODE_COMPANION_DISCOVERY
```

It replies with `kind: "drift-companion"`, `brand: "Drift"`, `name: "Drift"`, `protocol: "drift-remote"`, `version: 2`, the reachable `https://` URL, and `certificateSha256`, the authority fingerprint. Version 2 marks the move to HTTPS; companion apps should pin `certificateSha256` instead of accepting any certificate. Discovery never includes session tokens, passwords, or engine credentials.

## Architecture

```text
phone browser / WebView
        | TLS :41718 (plain HTTP there is redirected)
        v
Tauri-owned Remote Access gateway
        |-- /auth/*                 sign-in page API, device codes, password, certificate
        |-- /companion + /assets/*  embedded Vite dist (sign-in page when signed out)
        |-- /engine/*               the engine's own router, in process, with its token added
        |-- /api/invoke             explicit host-command allowlist
        |-- UDP :41717              credential-free address discovery
        v
the Drift engine (also on loopback for the desktop window)
```

- `src-tauri/src/remote.rs` owns listener lifecycle, the TLS accept loop and HTTP redirect, security headers, static assets, the engine mount, RPC dispatch, and discovery.
- `src-tauri/src/remote_auth.rs` owns devices, link codes, password sign-in, throttling, and the `/auth/*` handlers. The signed-out page is the self-contained `remote_sign_in.html`, English only because it loads before the app bundle.
- `src-tauri/src/remote_tls.rs` owns the certificate authority and per-address leaf configurations.
- `src/state/remote-access.ts` and `src/ui/settings-remote-access.tsx` hold the desktop management UI and the remote device's sign-out; `src/ui/remote-link-notice.tsx` shows the waiting-device notice.

`src/runtime.ts` detects `/companion`, and the native client then targets the gateway's `/engine` with no token of its own (`src/engine/native/target.ts`); `src/backend.ts` selects Tauri invoke on desktop or same-origin RPC remotely, and returns to the sign-in page when RPC answers 401. The Vite production output is embedded in the Rust binary with `rust-embed`. In debug builds the gateway checks the local `dist/` first, so run `bun run build` after frontend changes before testing Remote Access through a native development build.

## Security Model

Remote Access is for your own network. It is not an Internet-facing service.

- The engine never listens beyond `127.0.0.1`. The gateway serves its router in process (`native_engine`): after the device is signed in, it removes the device's cookies, adds the engine's random token itself, so no device ever holds it, and strips the `/engine` prefix. The event WebSocket carries a `Lease` the gateway cancels when the device's credentials change, so a signed-out device's socket closes at once, and an HTTP response still streaming is cut the same way.
- Everything except the sign-in routes (`/auth/options`, `/auth/link`, `/auth/link/{id}`, `/auth/login`, `/auth/certificate`) requires a device session. Signed-out navigations to `/` or `/companion` get the sign-in page; other requests get 401.
- Management commands (enable, link, revoke, password) are desktop-only Tauri commands and are not in the remote RPC allowlist.
- A request to the engine carries at most 64 MB (`drift_engine::api::MAX_REQUEST_BYTES`, the same limit the desktop window has; attachments ride in prompts as base64); other routes at most 10 MB. Same-origin `https` requests are expected.
- Host/Origin checks run before authentication. The host comes from HTTP/2's `:authority` or HTTP/1.1's `Host` header; browsers negotiate HTTP/2, so checking `Host` alone rejects every browser request.
- `no-referrer`, `nosniff`, frame restrictions, `no-store`, and a restrictive Permissions Policy are applied at the gateway.

Anyone signed in can operate the coding agent, read session data, invoke allowed host management functions, and act on host workspaces with the host user's permissions. Link only your own devices. Do not port-forward `41718` to the Internet; use a VPN such as Tailscale, whose addresses the certificate already covers.

Upgrading from the shared access key: the old `?token=` URL and its cookie no longer work. Open the address and link each device once.

## Remote Limitations

- Android WebView file inputs work and attachments are sent to the engine through the gateway.
- Clipboard, notifications, and microphone capture follow browser policy; they are most reliable once the certificate is installed.
- Opening a code file is an explicit action on the host. Adding a workspace remotely asks for a host filesystem path because a host-native folder dialog is not useful on the remote device.
- Desktop window controls and application updates are hidden remotely and remain host-only.

## Test Procedure

1. Build and verify the code:

   ```bash
   bun install
   bun run typecheck
   bun run test
   bun run build
   cargo test --manifest-path src-tauri/Cargo.toml
   ```

2. Run Drift natively with `bun run dev:shell`.
3. Enable **Settings > Remote Access**. Open the shown address on another device (or scan the QR code) and continue past the certificate warning.
4. Enter the device's code on the desktop. Confirm the device opens `/companion` and the full workspace/session UI hydrates, and that it appears under **Devices**.
5. Start a prompt on one device and confirm transcript and tool updates appear on both. Exercise permission and question replies, attachments, undo/redo, model selection, settings, storage, prompts, and MCP management.
6. Set a password, sign in from a private browser window, then change the password and confirm that window is signed out while the linked device is not.
7. Download and install the certificate on a device, verify its fingerprint, and confirm the warning is gone.
8. Visit `http://<address>:41718` and confirm the redirect to HTTPS.
9. Sign out a device from the desktop and confirm it returns to the sign-in page on its next action.
10. Send the UDP discovery probe from another device and confirm the response advertises the HTTPS address and fingerprint.
11. Disable Remote Access and confirm HTTPS access and discovery stop immediately.

Physical-device checks should cover Android Back behavior, display cutouts/safe areas, the software keyboard while composing, file selection, coarse-pointer menus, sleep/resume event stream recovery, and a `1280x800` tablet viewport.
