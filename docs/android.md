# Drift for Android

An Android app that drives the Drift running on your PCs, at home or away: conversations, prompts,
approvals and settings, with no certificate warning and with phone features a browser cannot offer.
It does not control the PC's screen or input. The browser version at `/companion` stays, behind its
own switch (`docs/remote.md`), for a device without the app.

## Decisions

- **Platform.** Android 8 (API 26) and up. Name "Drift", package `dev.drift.remote`, the desktop's
  icon. iOS is out until there is a Mac to build it on.
- **Shape.** A Tauri app in `src-android/` that bundles Drift's own SolidJS UI and none of the
  engine. Every request to a PC goes through Rust, which pins that PC's Remote Access certificate
  authority, so the WebView never makes a TLS connection and can never show a warning.
- **Shared code.** `crates/drift-pairing` holds what the PC and the app must agree on: the QR
  payload and the remote API version. It has no engine in it.
- **Reach.** The same network, Tailscale (the authority's name constraints already cover
  100.64/10), and from anywhere through OpenTunnel when the PC turns on **Reach from anywhere**. The
  app pins the tunnel certificate's key, so the relay cannot stand in for the PC.
- **PCs.** One app holds several PCs. A PC is identified by its authority fingerprint; its
  addresses (LAN, Tailscale) and its tunnel are only where to look for it, and each connection
  refreshes them from what the PC reports.
- **Linking.** Only by scanning the desktop's pairing code: it carries the addresses, the
  fingerprint, the tunnel and its key, the MAC for Wake-on-LAN and a one-time ticket, so scanning
  both trusts the PC and links the phone. There is no discovery and no typed code: a second QR code
  for the browser on the same page caught the scanner, and typed codes needed a trust-on-first-use
  connection and six words to compare.
- **Versions.** The PC reports a remote API version; the app refuses a different major version and
  says which side to update. The PC and the app are released together.
- **Security on the phone.** The device token is kept with the Android Keystore. Opening the app
  after 5 minutes in the background asks for the fingerprint, face or PIN (on by default).
- **Unreachable PC.** The last conversations stay readable under a reconnecting banner; nothing can
  be sent and nothing is queued.
- **Layout.** The existing responsive UI, fixed for the Android back gesture, safe areas, the
  keyboard over the composer, and a PC switcher.
- **Phone extras.** Dictation (recorded on the phone, transcribed by the PC's local model), a share
  target into a chosen conversation, camera and gallery attachments, cues when a turn ends or asks
  something, and Wake-on-LAN on the same network.
- **Background.** An opt-in foreground service keeps the connection and notifies when a turn ends
  or asks a permission or question. Off by default; no Firebase.
- **Shipping.** CI builds a signed APK for each GitHub release and the app offers new releases.
  During development, builds go to the phone over wireless `adb`.

## Phases

1. **The desktop side.** One-time pairing tickets (`POST /auth/pair`), the pairing QR in Settings,
   the remote API version and computer name in `/auth/options`, every reachable address, the LAN
   adapter's MAC and the tunnel in the pairing data.
2. **The Android shell.** The crate, the pinned-TLS proxy the UI's engine client and RPC go
   through, the PC list, QR linking, and reaching a PC from anywhere through its tunnel. Usable on a
   phone from here.
3. **Native polish.** Back gesture, safe areas, keyboard, biometric lock, the offline banner.
4. **Phone extras.** Dictation, share target, camera and gallery, cues, Wake-on-LAN.
5. **Background and shipping.** The notification service, then CI signing, release APKs and the
   update prompt.

## Pairing

The desktop shows a QR code holding a `drift://pair` URI:

```text
drift://pair?v=1&n=<computer name>&f=<authority SHA-256, 64 hex>&a=<address>,<address>&p=41718&m=<MAC, 12 hex>&h=<tunnel host>&k=<tunnel key SHA-256, 64 hex>&t=<ticket, 64 hex>
```

`h` and `k` come together, while the PC's tunnel is up; `a` may then be empty. The app tries every
address with the authority in `f` as its only trust anchor, and the tunnel trusting only a public
certificate whose key matches `k`, all at once. It sends `POST /auth/pair {"ticket", "name"}` to the
one that answers (a network address if any does within 400 ms of the tunnel), since the ticket works
once. The PC answers `{"token"}`, a device session token, which the app sends as
`Authorization: Bearer`. A ticket lasts 10 minutes, works once, and showing a new one replaces it.
Wrong tickets count toward the same lockout as wrong passwords.

## How the app works

```text
WebView  --http-->  proxy on 127.0.0.1:<random>  --https, PC's authority pinned-->  PC gateway :41718
                                                  \--https, tunnel key pinned-->  OpenTunnel  -->  PC gateway
         /app, /app/computers, /assets/*   served from the bundled Vite build
         /app/api/*                        the app's own: PCs, scanning, choosing, forgetting
         /engine/*, /api/*, /auth/me|logout  passed to the chosen PC with this phone's session
```

- `src-android/` is a Tauri app with no IPC: the WebView opens
  `http://127.0.0.1:<port>/app/enter?secret=<launch secret>`, which sets an `HttpOnly`,
  `SameSite=Strict` cookie. Every other request needs that cookie and the exact `Host`, so other
  apps on the phone and web pages that resolve a name to 127.0.0.1 cannot use the proxy. Loopback
  is a secure context, so the microphone and clipboard work without TLS.
- The UI is the companion: `src/runtime.ts` treats `/app` as a remote runtime whose engine is at
  `/engine` on the same origin, so the WebView needs nothing the browser companion does not. A
  signed-out device goes to `/app/computers` instead of the companion's sign-in page, and the
  device settings offer **Switch computer**.
- `/app/computers` is `app.html`, a second Vite entry (`src/app/`): linked PCs (open, forget), each
  marked when it can be reached from anywhere, and **Scan pairing code**.
- Each request to a PC goes through `tls::client`, which trusts one thing. On a network that is the
  pairing code's authority, and the leaf must be issued by it for the address connected to. Through
  the tunnel it is a certificate that chains to Mozilla's roots (`webpki-root-certs`), is valid for
  the tunnel's name, and carries the pinned key. The event WebSocket is passed through as an HTTP
  upgrade, so the PC answers the WebView's own handshake.
- Linked PCs are `computers.json` in the app's private storage: the authority fingerprint, every
  known address (the last that answered first), the port, the MAC, the tunnel and its key, and the
  device token. Opening the app reconnects to the last PC used, trying every address and the tunnel
  at once and preferring the network; it then reads `reach` from `GET /auth/me` and keeps the PC's
  current addresses and tunnel, over a connection that already trusted the PC. When nothing
  answers, or the PC signed the phone out, the computers page says why, and tells a PC without a
  tunnel how to reach it from anywhere. A request that cannot reach the PC drops the connection, and
  the next one looks for the PC again, so a page left open while the phone slept or moved from
  Wi-Fi to mobile data recovers by itself, through the tunnel. Moving the token into the Android
  Keystore comes with the biometric lock (phase 3).
- `MainActivity.kt` draws edge to edge with light bar icons over drift-dark's background, and pads
  the WebView by the status bar, navigation bar, cutout and keyboard insets, so the page, its
  drawers and dialogs, and the composer while typing all sit in the space left; `env(safe-area-*)`
  is then 0 inside the page.
- Below 720 CSS pixels, or on touch screens at most 500 pixels tall, settings use a full-screen labeled section list with Back navigation and
  search. The desktop keeps its sidebar and dialog. Every row follows one rule: a switch stays beside
  its text, any other control (field, picker, slider, segmented choice, buttons) takes the full line
  below it. Touch controls have 44-pixel targets with their content centred, and hover-only buttons
  are always shown.
- The phone composer keeps its pickers and buttons on one row under the text, as on the desktop, with
  36-pixel buttons, and dock cards tuck under it. Pickers open as dismissible sheets without
  opening the keyboard. Touch Enter inserts a newline; Send or Ctrl+Enter submits. Textarea height
  follows the available viewport, and approval cards scroll above it instead of hiding Send.
- A reply's footer stays on one line on a phone: the model name truncates before Copy or Fork can wrap.
- The drawer follows horizontal drags from anywhere, thread and workspace rows included, except from
  fields, sliders, dialogs, menus or horizontally scrollable content; a thumb's curved swipe still
  counts, and the row a swipe ends on is not opened. Its first 24 pixels are left to Android Back.
  Multiple fingers cancel a drawer drag. Threads expose a visible menu, shown as a bottom sheet, rather
  than requiring hover or long-press.
- Dialogs and picker sheets consume Android Back before the conversation route changes. File
  previews fill the phone screen, image controls wrap, transcript search fits the header, and
  notices stay above the composer. These layout changes do not apply to the desktop breakpoint.
- The computers page scrolls with the keyboard open, shows loading and retry states, and confirms
  Forget before unlinking a PC.
- `DriftPlugin.kt` is the only other Kotlin: Google's code scanner (`play-services-code-scanner`), which
  shows its own camera screen and needs no camera permission, and the phone's name for the PC's
  device list. Cleartext is allowed only to 127.0.0.1 (`network_security_config.xml`).

## Building

The Gradle project is `src-android/gen/android`, generated by `tauri android init` and kept in the
repository; Drift's icons replace the generated ones, and its `BuildTask.kt` calls the Tauri CLI
through `bun x`. With the Android SDK, NDK 28 (`NDK_HOME`) and JDK 21:

```bash
cd src-android
bunx tauri android build --debug --apk --target aarch64
adb install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

The build runs `bun run build` first; the proxy embeds `dist/` in debug builds too.
