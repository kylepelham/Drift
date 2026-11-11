# Remote Access

Remote Access lets you use the Drift on this computer from your phone, or from a browser on another device: the
same engine, conversations, workspaces, settings and voice transcription as the desktop app.

Remote Access is off by default. Every connection is encrypted, and only devices you link can use it.

## What is on the page

**Settings > Remote Access** has one switch that turns everything on, then three parts:

- **Connect a device.** How a phone (`docs/android.md`) or another computer connects, and the addresses to type when
  this computer is not listed nearby. The device asks; this computer's user accepts or declines.
- **Connection.** **Reach from anywhere** connects this computer to OpenTunnel so the app works away from its network.
  **Browser access** turns on the browser version, which signs in with a username and password.
- **Devices.** Every signed-in device, how it signed in and when it was last active, each with **Sign out**.

## Linking a device

A phone or another computer links in three steps: it finds this PC, asks to connect, and this PC's user accepts.

- **Finding.** While Remote Access is on, this PC answers on UDP `41718` (`remote/discovery.rs`) when a device on one
  of its networks broadcasts `drift-discover/1`, with its name, the SHA-256 of its certificate authority (its id) and
  the gateway's port. The device lists every computer that answers; a computer the device already links is left out.
  Where broadcasts do not reach, such as Tailscale, the device types an address or name instead; **Connect a device**
  shows this PC's addresses.
- **Asking.** The device connects trusting whatever authority the gateway shows, and only that one from then on
  (`tls::first_use`). `POST /auth/request {"name"}` answers with a request id and this PC's commitment, the SHA-256 of
  a random secret. The device then sends its own random secret to `/auth/request/reveal`, and only then does this PC
  reveal its own. Both derive a six-digit code from the authority the device met and the two secrets
  (`drift_pairing::matching_code`), and the device refuses a secret that does not match the commitment.
- **Accepting.** The request shows on this PC from the moment both secrets are in: a prompt that names the device, its
  address and the code, with **Accept** and **Decline**, over this PC's own Drift, and the window is brought up.
  While the window shows another computer the prompt waits for **This PC**, within the request's two minutes.
  The device shows the same code while it waits on `/auth/request/wait`, a request held open up to 25 seconds at a
  time. Accepted, it gets a session token it sends as `Authorization: Bearer`, and is listed as **Drift app**.
  Declined, the device is told so, and the decline counts toward the same lockout as a wrong password. A request runs
  out after two minutes, and the device's **Cancel** withdraws it (`/auth/request/cancel`).

The code is what makes first use safe. A device in the middle of the network would meet the asking device with an
authority of its own, which changes the device's code, and it cannot pick a secret to make the two codes agree: this
PC commits to its secret before it sees the device's, and the device sends its secret before it sees this PC's. So
accept only when both screens show the same code. A new request from an address replaces its last, and at most three
wait at once.

The request routes answer only on this PC's own networks: private, link-local and Tailscale addresses, never through
the tunnel, so nobody on the internet can make a prompt appear. Once linked, a device reaches this PC from anywhere.

`GET /auth/me` also answers with `reach`: the addresses, port, tunnel and LAN adapter's MAC (for Wake-on-LAN) the PC
has now. The device takes it right after it is accepted and over each connection it trusts, so it learns the tunnel
without having been away, and a new DHCP lease or a tunnel turned on later needs no new link.

## Driving another computer from this one

The desktop drives another computer's Drift the way the phone does, in its own main window. On the computer to drive,
turn Remote Access on. On this PC, open the computer menu at the top of the sidebar (the Drift brand, which names the
computer shown), choose **Link or manage computers...**, and pick it under **Nearby**, or type its address. Once the
other computer's user accepts, it opens. The other computer lists this PC by its name, as a **Drift app** device, and
neither ever lists itself.

The menu switches the window between **This PC** and each linked computer. For quicker switching, **Show as tabs in
the sidebar** in the menu puts them in a row of tabs under it, one click each, with a computer that does not answer
dimmed (checked every minute). The choice is kept in `computers.json`, so it holds whichever computer the window shows.
**Ctrl+Alt+1** to **9** open the computer at that place in the menu, This PC being 1, and **Ctrl+Alt+Right** and
**Ctrl+Alt+Left** step to the next or previous one, rebindable under **Shortcuts**. The arrows are the default because
AltGr is Ctrl+Alt on Windows, and many layouts type brackets with it. The other computer's Drift is served by
`src-tauri/src/computers.rs` through the same proxy as the phone's (`crates/drift-remote-client`): a server on
127.0.0.1, started on first use, that serves this build's UI and passes `/engine`, `/api` and `/auth` to the computer
over TLS that trusts only its authority or tunnel key, adding this PC's session. The window keeps the page it left and
returns to it, so this PC's Drift picks up where it was, and its engine keeps running meanwhile. Linked computers are
kept in `computers.json` in the app data folder. Opening a computer that cannot be reached leaves the window where it
was and says why. One that stops answering while it is open shows the connection error with **Retry** and **Switch
computer**, which opens the computers page: **This PC**, the linked computers, and the computers nearby.

A page from the proxy runs as a remote one (`/app`), so `src/shell.ts` hands it the window controls and zoom only,
never this PC's commands, engine or events. The `remote-computer` capability grants `http://127.0.0.1:*` exactly
those: minimize, maximize, close, drag, zoom, and the event listening the window's resize state needs.

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
  It could not have one for this PC's key, which never leaves the PC and which renewals keep. `/auth/me` carries
  the SHA-256 of that key (from the certificate's SubjectPublicKeyInfo), and the app refuses a
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
must permit inbound TCP `41718` for the network, and UDP `41718` for this computer to be found nearby. The tunnel needs no inbound port: Drift connects out to OpenTunnel.

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
   |-- /auth/*                 options, connect requests, password sign-in, session, certificate
   |-- /companion + /assets/*  embedded Vite dist (sign-in page when signed out), browser access only
   |-- /engine/*               the engine's own router, in process, with its token added
   |-- /api/invoke             explicit host-command allowlist
   v
the Drift engine (also on loopback for the desktop window)
```

- `src-tauri/src/remote.rs` owns the switches, listener lifecycle, status and the remote RPC allowlist;
  `remote/transport.rs` the TLS accept loop and HTTP redirect; `remote/gateway.rs` the routes, browser gating, host
  checks, security headers, static assets and the engine mount; `remote/discovery.rs` the answers to nearby devices;
  `remote/tunnel.rs` OpenTunnel, the PROXY header and the key pin.
- `src-tauri/src/remote_auth.rs` owns devices, password sign-in, throttling, and the `/auth/*` handlers;
  `remote_auth/requests.rs` the devices asking to connect. The signed-out page is the self-contained `remote_sign_in.html`, English only because it loads before the
  app bundle.
- `src-tauri/src/remote_tls.rs` owns the certificate authority and per-address leaf configurations.
- `crates/drift-pairing` holds what both sides agree on: the discovery messages, the commitment and the matching code.
- `src/state/remote-access.ts` and `src/ui/settings-remote-access.tsx` hold the desktop management UI and the remote
  device's sign-out; `src/ui/connect-requests.tsx` the accept prompt; `src/ui/connect-computer.tsx` the asking side,
  shared by the desktop's computers dialog and the app's computers page.

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
- Everything except `/auth/options`, `/auth/login`, `/auth/request*` and `/auth/certificate` requires a device
  session.
  `/auth/options` reports whether password sign-in is set, the remote API version, the app version and the
  computer's name. Signed-out navigations to `/` or `/companion` get the sign-in page; other requests get 401.
- Management commands (the switches, answering connect requests, sign-out, password) are desktop-only Tauri commands
  and are not in the remote RPC allowlist.
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
is on. The QR pairing code and its one-time ticket (`/auth/pair`) of 2.4 to 2.5 are gone; devices linked with them
keep their sessions.

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
3. Turn on **Settings > Remote Access**. In the Drift app on the same network, pick this computer under **Nearby**,
   confirm both screens show the same code, and accept. Confirm the phone opens Drift and appears under **Devices**.
   Ask again and decline, and confirm the phone says so.
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
