# Remote Access

Remote Access lets you use the Drift on this computer from your phone, or from a browser on another device: the
same engine, conversations, workspaces, settings and voice transcription as the desktop app.

Remote Access is off by default. Every connection is encrypted, and only devices you link can use it.

## What is on the page

**Settings > Remote Access** has one switch that turns everything on, then three parts:

- **Drift for Android.** **Show pairing code** shows a QR code for the Drift app (`docs/android.md`). Scanning it
  links the phone. This is the only way a device links; there is no typed code.
- **Connection.** **Reach from anywhere** connects this computer to OpenTunnel so the app works away from its network.
  **Browser access** turns on the browser version, which signs in with a username and password.
- **Devices.** Every signed-in device, how it signed in and when it was last active, each with **Sign out**.

## Linking a phone

The pairing code holds this PC's name, every private and Tailscale address it has (the LAN address first), the
fingerprint of its certificate authority, the LAN adapter's MAC for Wake-on-LAN, the tunnel's host and key while
**Reach from anywhere** is on, and a one-time ticket. The app trusts only that authority (and, through the tunnel,
only that key), so it never shows a certificate warning, and redeems the ticket with
`POST /auth/pair {"ticket", "name"}` for a session token it sends as `Authorization: Bearer`. A ticket works once, for
10 minutes; showing a new code replaces it, and wrong tickets count toward the same lockout as wrong passwords. The
device is listed as **Drift app**. The page says whether the code shown reaches the PC from anywhere or only on its
network.

`GET /auth/me` also answers with `reach`: the addresses, port and tunnel the PC has now. The app takes it over each
connection it trusts, so a new DHCP lease or a tunnel turned on later needs no new pairing.

## Reach from anywhere

With **Reach from anywhere** on, Drift connects to [OpenTunnel](https://github.com/anomalyco/opentunnel), the relay
opencode uses for the same purpose. OpenTunnel is a blind TLS tunnel: it passes the encrypted bytes of each connection
through and never holds the certificate's private key, so it cannot read the traffic.

- **Setup.** The first time, Drift creates a tunnel (`<id>.opentunnel.xyz`) and waits for its certificate, which
  takes up to a minute; the page shows **Setting up**. The identity lives in the app data directory under
  `opentunnel/default/`, not in the shared XDG directory, which Windows lacks. An interrupted setup resumes next time.
- **Address.** Drift claims a random 16-character route on the tunnel, kept in `app_setting` (`remote_tunnel_route`),
  so the address is `https://<route>.<id>.opentunnel.xyz` and stays the same across restarts. Tunnel hostnames appear
  in public certificate logs; route names never do, because the certificate is a wildcard. The route is not
  authentication: everything behind it still needs a signed-in device.
- **The key pin.** The relay controls DNS for `opentunnel.xyz`, so it could have a certificate issued for the address.
  It could not have one for this PC's key, which never leaves the PC and which renewals keep. The pairing code and
  `/auth/me` carry the SHA-256 of that key (from the certificate's SubjectPublicKeyInfo), and the app refuses a
  certificate without it, so even the relay cannot stand in for the PC.
- **Status.** Settings shows Setting up, Connecting, Online with the address, Reconnecting, or Could not connect with
  the reason and **Try again**. The crate reconnects through network failures by itself and picks up renewed
  certificates; only a rejected token or a failed certificate stops it.
- **Off.** Turning it off disconnects the tunnel and keeps its identity, so turning it on again is immediate and
  phones keep their pin.

The tunnel's client terminates TLS in Drift and hands each connection to a listener on `127.0.0.1` as plain HTTP,
after a PROXY protocol v2 header naming the visitor's address. That address drives sign-in throttling, so a stranger
cannot lock everyone out at once. Nothing is trusted for coming from loopback; a tunneled request must name the
tunnel's own host exactly, or it is refused.

## Browser access

**Browser access** is off by default. While it is on:

- The device opens the address shown (on this network, `https://192.168.1.20:41718`; with the tunnel online, the
  tunnel's address too) and gets the sign-in page. Typing it without `https://` also works on the network: plain HTTP
  on the same port redirects.
- Sign-in is by username and password, set under **Browser access** (8 to 256 characters). Until one is set, the page
  says so instead of showing the form. Changing or turning off the password signs out every browser that used it.
- Browsers hold a session cookie; the app never uses one.

While it is off, the browser version and its sign-in answer 404, and no session cookie is accepted, so a browser left
open loses its event stream at once. The Drift app is unaffected: it has its own UI and uses only the API routes.

Passwords are stored as PBKDF2-HMAC-SHA256 with a random 16-byte salt and 600,000 iterations, in Drift's SQLite
`app_setting` table. After five wrong attempts from one address, sign-in from that address locks for 30 seconds,
doubling per further failure up to 15 minutes. At most two password checks run at once.

Each device holds its own random 256-bit session token, in an `HttpOnly; Secure; SameSite=Strict` cookie that lasts
400 days for a browser, or as a bearer token for the app. Drift stores only the token's SHA-256 in the
`remote_device` table, so the database never contains a usable credential. **Sign out** revokes one device; **Sign
out all devices** revokes all of them. Revocation closes open streams and the engine event socket immediately. A
remote device can also sign itself out from its own **Settings > Remote Access**.

## Encryption on the network

The gateway only speaks TLS (1.2 or 1.3, HTTP/2 or HTTP/1.1) on TCP `41718`. On first use Drift creates a private
certificate authority for this computer and keeps it in the app data directory (`remote-tls/`). For each local
address a device connects to, Drift issues a leaf certificate signed by that authority, so changing Wi-Fi networks or
DHCP leases needs no manual steps.

The Drift app pins this authority. Browsers do not know it, so on the network address they warn on first connect;
the tunnel's address has a public certificate and does not. To stop the warning, download the certificate from the
sign-in page (`/auth/certificate`) and install it as a trusted certificate authority, after checking its SHA-256
fingerprint matches the one under **Browser access > Local certificate**. The authority carries X.509 name
constraints that only permit private addresses (10/8, 172.16/12, 192.168/16, 100.64/10 including Tailscale,
169.254/16, 127/8, `localhost`, and `.local`), so trusting it cannot be abused to impersonate public websites even if
its key were stolen. A trusted certificate also makes the page a secure context, which browsers require for the
microphone and the clipboard.

If Windows asks whether Drift may accept network connections, allow it on private networks only. A host firewall
must permit inbound TCP `41718` for the network. The tunnel needs no inbound port: Drift connects out to OpenTunnel.

## Architecture

```text
Drift app / browser
   |                          \
   | TLS :41718 (private CA)   \ TLS to <route>.<id>.opentunnel.xyz (public cert, key pinned by the app)
   v                            v
   |                         OpenTunnel relay, passes TLS through unread
   |                            |  bridge WebSocket, opened by Drift
   v                            v
Tauri-owned Remote Access gateway   <- tunnel client: TLS ends here, then PROXY v2 + HTTP on 127.0.0.1
   |-- /auth/*                 options, pairing, password sign-in, session, certificate
   |-- /companion + /assets/*  embedded Vite dist (sign-in page when signed out), browser access only
   |-- /engine/*               the engine's own router, in process, with its token added
   |-- /api/invoke             explicit host-command allowlist
   v
the Drift engine (also on loopback for the desktop window)
```

- `src-tauri/src/remote.rs` owns the switches, listener lifecycle, status, pairing and the remote RPC allowlist;
  `remote/transport.rs` the TLS accept loop and HTTP redirect; `remote/gateway.rs` the routes, browser gating, host
  checks, security headers, static assets and the engine mount; `remote/tunnel.rs` OpenTunnel, the PROXY header and
  the key pin.
- `src-tauri/src/remote_auth.rs` owns devices, the pairing ticket, password sign-in, throttling, and the `/auth/*`
  handlers. The signed-out page is the self-contained `remote_sign_in.html`, English only because it loads before the
  app bundle.
- `src-tauri/src/remote_tls.rs` owns the certificate authority and per-address leaf configurations.
- `crates/drift-pairing` holds the pairing code both the PC and the app read.
- `src/state/remote-access.ts` and `src/ui/settings-remote-access.tsx` hold the desktop management UI and the remote
  device's sign-out.

`src/runtime.ts` detects `/companion`, and the native client then targets the gateway's `/engine` with no token of its
own (`src/engine/native/target.ts`); `src/backend.ts` selects Tauri invoke on desktop or same-origin RPC remotely,
and returns to the sign-in page when RPC answers 401. The Vite production output is embedded in the Rust binary with
`rust-embed`. In debug builds the gateway checks the local `dist/` first, so run `bun run build` after frontend
changes before testing Remote Access through a native development build.

## Security model

- The engine never listens beyond `127.0.0.1`. The gateway serves its router in process: after the device is signed
  in, it removes the device's cookies, adds the engine's random token itself, so no device ever holds it, and strips
  the `/engine` prefix. The event WebSocket carries a `Lease` the gateway cancels when the device's credentials
  change, so a signed-out device's socket closes at once, and an HTTP response still streaming is cut the same way.
- Everything except `/auth/options`, `/auth/login`, `/auth/pair` and `/auth/certificate` requires a device session.
  `/auth/options` reports whether password sign-in is set, the remote API version, the app version and the
  computer's name. Signed-out navigations to `/` or `/companion` get the sign-in page; other requests get 401.
- Management commands (the switches, pairing codes, sign-out, password) are desktop-only Tauri commands and are not
  in the remote RPC allowlist.
- A request to the engine carries at most 64 MB (`drift_engine::api::MAX_REQUEST_BYTES`, the desktop window's limit);
  other routes at most 10 MB.
- Host and Origin checks run before authentication: the Origin, when sent, must be `https` on the same host. The host
  comes from HTTP/2's `:authority` or HTTP/1.1's `Host` header. Through the tunnel the host must also be the tunnel's
  own address.
- `no-referrer`, `nosniff`, frame restrictions, `no-store`, and a restrictive Permissions Policy are applied at the
  gateway.

Anyone signed in can operate the coding agent, read session data, invoke allowed host management functions, and act
on host workspaces with the host user's permissions. Link only your own devices. Do not port-forward `41718` to the
Internet; use **Reach from anywhere** or a VPN such as Tailscale, whose addresses the certificate already covers.

Upgrading: devices linked with a typed code before 2.4 are still listed and sign in as browsers while browser access
is on. The UDP discovery probe on `41717` is gone; the app finds PCs only through their pairing code.

## Remote limitations

- Android WebView file inputs work and attachments are sent to the engine through the gateway.
- Clipboard, notifications, and microphone capture follow browser policy; on the network address they are most
  reliable once the certificate is installed.
- Opening a code file is an explicit action on the host. Adding a workspace remotely asks for a host filesystem path
  because a host-native folder dialog is not useful on the remote device.
- Desktop window controls and application updates are hidden remotely and remain host-only.

## Test procedure

1. Build and verify the code:

   ```bash
   bun install
   bun run gates
   bun run build
   ```

2. Run Drift natively with `bun run dev:shell`.
3. Turn on **Settings > Remote Access**, show the pairing code, and scan it with the Drift app on the same network.
   Confirm the phone opens Drift, appears under **Devices**, and the page says it is linked.
4. Turn on **Reach from anywhere** and wait for **Online**. On the phone, turn Wi-Fi off and confirm Drift keeps
   working over mobile data, then back on.
5. Start a prompt on one device and confirm transcript and tool updates appear on both. Exercise permission and
   question replies, attachments, undo and redo, model selection and settings.
6. Turn on **Browser access**, set a password, open the network address in a private browser window and sign in; with
   the tunnel online, open its address too and confirm there is no certificate warning.
7. Turn **Browser access** off and confirm the browser window loses its connection and the address answers 404, while
   the phone keeps working.
8. Change the password and confirm only browsers that used it are signed out.
9. Sign out a device from the desktop and confirm it returns to sign-in (or the app's computers page) on its next
   action.
10. Turn Remote Access off and confirm the network address and the tunnel stop at once.
