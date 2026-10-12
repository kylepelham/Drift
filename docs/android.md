# Drift for Android

An Android app that drives the Drift running on your PCs: conversations, prompts, approvals and
settings, as the browser companion does, but with no certificate warning and with phone features
the browser cannot offer. It does not control the PC's screen or input. The browser companion at
`/companion` stays as the fallback for a device without the app.

## Decisions

- **Platform.** Android 8 (API 26) and up. Name "Drift", package `dev.drift.remote`, the desktop's
  icon. iOS is out until there is a Mac to build it on.
- **Shape.** A Tauri app in `src-android/` that bundles Drift's own SolidJS UI and none of the
  engine. Every request to a PC goes through Rust, which pins that PC's Remote Access certificate
  authority, so the WebView never makes a TLS connection and can never show a warning.
- **Shared code.** `crates/drift-pairing` holds what the PC and the app must agree on: the QR
  payload, the fingerprint words and the remote API version. It has no engine in it.
- **Reach.** The same network, and Tailscale (the authority's name constraints already cover
  100.64/10). No relay.
- **PCs.** One app holds several PCs. A PC is identified by its authority fingerprint; every
  address it offered (LAN, Tailscale) is kept, the last that worked is tried first, and discovery
  and each connection refresh the list.
- **Linking.** Discovery lists PCs on the network that are not linked yet. Linking is a QR scan of
  the desktop's Remote Access page: the code carries the addresses, the fingerprint, the MAC for
  Wake-on-LAN and a one-time ticket, so scanning both trusts the PC and links the phone. Discovery
  replies are unauthenticated UDP, so the app never trusts a fingerprint from discovery alone.
  The fallback when the camera cannot scan is the existing typed code, with a fingerprint shown as
  six words on both screens to compare before pressing Link. Six words of 2048 are 66 bits: four
  would be 44, few enough for an attacker on the network to find a certificate whose fingerprint
  shares them in under an hour of GPU hashing.
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
   the remote API version and computer name in `/auth/options` and discovery, the fingerprint
   words on the desktop and beside a waiting typed-code link, every reachable address and the LAN
   adapter's MAC in the pairing data.
2. **The Android shell.** The crate, the pinned-TLS proxy the UI's engine client and RPC go
   through, the PC list, discovery, QR and typed-code linking. Usable on a phone from here.
3. **Native polish.** Back gesture, safe areas, keyboard, biometric lock, the offline banner.
4. **Phone extras.** Dictation, share target, camera and gallery, cues, Wake-on-LAN.
5. **Background and shipping.** The notification service, then CI signing, release APKs and the
   update prompt.

## Pairing

The desktop shows a QR code holding a `drift://pair` URI:

```text
drift://pair?v=1&n=<computer name>&f=<authority SHA-256, 64 hex>&a=<address>,<address>&p=41718&m=<MAC, 12 hex>&t=<ticket, 64 hex>
```

The app connects to an address with the authority in `f` as its only trust anchor, then sends
`POST /auth/pair {"ticket", "name"}`. The PC answers `{"token"}`, a device session token like a
linked browser's, which the app sends as `Authorization: Bearer`. A ticket lasts 10 minutes, works
once, and showing a new one replaces it. Wrong tickets count toward the same lockout as wrong
passwords.

## How the app works

```text
WebView  --http-->  proxy on 127.0.0.1:<random>  --https, PC's authority pinned-->  PC gateway :41718
         /app, /app/computers, /assets/*   served from the bundled Vite build
         /app/api/*                        the app's own: PCs, discovery, scanning, linking
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
- `/app/computers` is `app.html`, a second Vite entry (`src/app/`): linked PCs (open, forget),
  **Scan pairing code**, discovery of PCs not linked yet, and linking by typed code from a
  discovered PC or a typed address (for Tailscale, where broadcasts do not reach).
- Each request to a PC goes through `tls::client`, which trusts one authority: the pairing code's,
  or for a typed-code link the first one met, held from then on. The leaf must still be issued by
  it for the address connected to. The event WebSocket is passed through as an HTTP upgrade, so
  the PC answers the WebView's own handshake.
- Linked PCs are `computers.json` in the app's private storage: the authority fingerprint, every
  known address (the last that answered first), the port, the MAC and the device token. Opening the
  app reconnects to the last PC used, trying its addresses in order; when none answers, or the PC
  signed the phone out, the computers page says why. Moving the token into the Android Keystore
  comes with the biometric lock (phase 3).
- `MainActivity.kt` draws edge to edge with light bar icons over drift-dark's background, and pads
  the WebView by the status bar, navigation bar, cutout and keyboard insets, so the page, its
  drawers and dialogs, and the composer while typing all sit in the space left; `env(safe-area-*)`
  is then 0 inside the page.
- On a phone the composer's pickers take a row of their own above its buttons, the sidebar drawer
  closes with a leftward swipe (a third of its width, or a flick), and any question card folds to
  its header like the Todos dock, in the desktop app too. There is no swipe to open the drawer:
  the left edge is Android's back gesture.
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
