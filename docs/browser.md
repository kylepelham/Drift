# Browser

Each conversation can have one browser the agent tests in. The engine starts it, owns every process it
runs, and streams it into a browser pane beside the chat. The pane opens when the agent starts testing;
collapsing it stops the stream, not the browser, so testing carries on unseen. Going to another
conversation and back finds the pane as it was, for as long as the browser is still running.

This replaces the chrome-devtools and web-test MCP servers for everyday testing: no browser windows open
across the desktop, nothing outlives Drift, and remote devices see the same pane.

## Status

| Part | State |
| --- | --- |
| Chrome and Edge over the DevTools Protocol, per-conversation lifecycle, `browser` tool, pane with stream, tabs and input | Built |
| Settings: tool on or off, Chrome or Edge, isolated or shared profile, hidden or off-screen window | Built |
| Project browser config (`drift.json` `browser`), keychain credentials, Windows integrated sign-in | Planned |
| IE mode through IEDriverServer, web-test's app profiles (frames, VBScript, ExtJS) | Planned |
| Firefox over WebDriver BiDi | Planned |
| Camoufox through a Drift-owned Playwright driver | Planned |
| Performance traces and Lighthouse | Not planned; the chrome-devtools MCP still does these |

## How it fits together

```text
browser tool / pane input
        -> engine browser module (one instance per conversation)
        -> driver: DevToolsProtocol (Chrome, Edge) | BiDi (Firefox) | WebDriver (IE mode) | Playwright (Camoufox)
        -> browser process tree, held in a Windows job object
frames  <- screencast (Chromium) or polled screenshots (Firefox, IE mode)
        -> /sessions/{id}/browser/view WebSocket, only while a pane watches
```

The engine module is `crates/drift-engine/src/browser`. Its drivers are written against the engine's
existing `tokio-tungstenite` and `reqwest`; Chrome and Edge need no new dependency. The pane is
`src/ui/browser-pane.tsx`.

The page never runs in Drift's own webview. The pane shows images of it and sends the user's input back,
so a hostile page cannot reach the UI, the engine token or the shell. This is also why the same pane
works from a phone.

## Lifecycle

- A conversation's browser starts the first time its agent calls `browser` with `open` (or any action that
  needs a page). Subagents are conversations too: each gets its own, which ends with its run.
- It keeps running after the turn ends, after Stop, while the pane is collapsed, and while the user looks at
  another conversation.
- It ends when the agent calls `stop`, the user presses Stop in the pane, the conversation is archived or
  deleted, Drift quits, or it has been idle for 30 minutes (no tool call and no pane watching).
- The browser and everything it starts live in one job object on Windows (a process group elsewhere), as
  background shells do, so a crash of Drift cannot leave one behind.
- A browser that exits on its own (a crash) is reported as stopped; the next `open` starts a fresh one.

## Profiles and disk

Profiles live under Drift's data folder, named for what they are:

```text
browsers/
  sessions/<conversation id>/<engine>/     isolated: one per conversation, deleted when its browser ends
  shared/<workspace name>-<id>/<engine>/   shared: one per workspace and engine, kept between conversations
```

- **Isolated is the default.** A conversation starts signed out and leaves nothing behind.
- **Shared** keeps sign-ins for a workspace across its conversations, chosen in Settings. Two conversations
  of the same workspace share one profile, so Chrome would refuse the second launch; the second gets an
  isolated profile instead and its result says so.
- Chrome's caches are capped (`--disk-cache-size` 50 MB) and its background downloads (component and
  optimization model updates, Safe Browsing lists, sync) are turned off, so a profile stays a few MB.
- When a shared profile's browser ends, its caches (HTTP, code, GPU and shader caches, service worker
  cache storage) are deleted; cookies, local storage and IndexedDB stay.
- At startup and in housekeeping, isolated profiles with no running browser (left by a crash) are deleted,
  as are shared profiles of workspaces Drift no longer has.

## Getting blocked less

The goal is that ordinary sites treat the browser as a person's, without giving up a feature for it. What
cannot be done without losing a feature is left to Camoufox.

- The real installed Chrome or Edge is used, never Chromium or Chrome for Testing. If the one Settings names
  is missing, the other is used and the result says so.
- No `--enable-automation`, and `--disable-blink-features=AutomationControlled`, so `navigator.webdriver`
  is false and no automation banner or infobar exists.
- In hidden (headless) mode the user agent and client hints are set to the real browser's, without
  `HeadlessChrome`, on every page and popup before it runs a script.
- Clicks, typing and scrolling are real input events (`Input.dispatch*`), never `element.click()`, so they
  are trusted events with real coordinates.
- Snapshots and ref lookups run in an isolated world, invisible to the page's own scripts.
- The off-screen window mode runs a normal window with the real GPU, for sites that check rendering; it
  shows in the taskbar. Window occlusion and background throttling are turned off so it still renders.
- Console capture needs `Runtime.enable`, which a few anti-bot scripts detect. It stays on, because
  console errors are half of what testing is for; Camoufox is the answer for such sites.

## The `browser` tool

One tool with an `action`, like `process`, so the schema stays small. Every action except `open`,
`stop` and `pages` acts on the active page unless `page` names another.

| Action | Does |
| --- | --- |
| `open` | Starts the browser if needed and opens `url` in a new tab (or the first blank one). |
| `navigate` | Loads `url` in the page, or `back`, `forward`, `reload`. |
| `snapshot` | The page as an accessibility-style tree, each element it can act on tagged `[ref=N]`. Password, card and one-time-code values are never shown. |
| `click`, `hover` | By `ref`, or by `x`/`y` in the last screenshot's CSS pixels. `double` for a double click. |
| `fill` | Clears and types `text` into an input, textarea or editable element by `ref`; a `select` gets the option whose value or label matches. Checkboxes take `checked`. |
| `type`, `press` | Text into the focused element; a key or chord such as `Enter`, `Control+A`. |
| `scroll` | By `ref` into view, or `dy` pixels. |
| `wait` | Until `text` appears or `ms` pass, at most 30 s. |
| `screenshot` | A JPEG of the viewport for the model to look at. |
| `eval` | JavaScript in the page; the result as JSON. |
| `logs` | Console messages or network requests since the page loaded, optionally matching `search`; `body` with a request id returns its response body. |
| `dialog` | Accepts or dismisses an open alert, confirm or prompt, with optional `text`. |
| `viewport` | `desktop` (the browser's own 1280x800 window), `tablet` (768x1024), `mobile` (390x844, touch and a phone's user agent), or `width`/`height`; the page reloads so it lays out again. |
| `pages`, `select`, `close` | Lists tabs (popups included), makes one active, closes one. |
| `batch` | Runs `actions`, a list of the above, in order in one call, stopping at the first failure. |
| `stop` | Ends this conversation's browser. |

Results say what changed (the page's title and address after an action that navigated, a dialog that
opened, a popup that appeared) so the model does not need to snapshot after every click.

Permission: acting on a page asks once per host (`browser` permission, the host as its pattern), like
Claude's site approval. Loopback and private addresses are allowed without asking, as a dev server is
the common case. An "always" answer is kept for the workspace.

## API

- `GET /sessions/{id}/browser` the conversation's browser, or `null`.
- `POST /sessions/{id}/browser/stop` ends it.
- `GET /sessions/{id}/browser/view` a WebSocket. The engine sends binary messages, each a 4-byte
  little-endian header length, a JSON header (`page`, `width`, `height`) and a JPEG. The client sends JSON
  text: `mouse`, `key`, `text`, `select`, `close`, `navigate`. The stream runs only while at least one view
  socket is open, and only for the active page.
- Event `browser.updated` carries the whole record (state, engine, storage, pages with their address,
  title and which is active). It is told only once the browser is listed, so a client never hears of a
  browser `GET` cannot return yet.
- Chrome does not report a title a page's script sets, so tab titles are read again after each load, after
  each agent action, and every 2 s while a pane watches.
- A screencast sends nothing until the page paints again, so each new watcher, and each tab the stream
  moves to, first gets a screenshot as its first frame.

## The pane

- Opens by itself when a conversation's browser starts, unless the user collapsed it for that conversation.
- Tabs for the browser's pages, popups included; an address bar with back, forward and reload; Stop ends
  the browser, the collapse button only hides the pane.
- The page image keeps its aspect ratio; clicks, wheel and keys typed while it has focus go to the page,
  mapped to its CSS pixels. Pasting sends the text as typed input.
- A collapsed browser shows as a button in the chat header while it runs.

## Planned

### Project browser config

A `browser` section in `drift.json`, the replacement for web-test's `projects.json`:

```jsonc
{
    "browser": {
        "engine": "ie",                    // chrome | edge | firefox | camoufox | ie
        "baseUrl": "http://localhost/Traveller/",
        "engines": { "ie": ["*.asp"] },    // URL patterns that switch engine
        "storage": "shared",               // isolated | shared
        "credentials": { "admin": { "user": "kyle", "secret": "traveller-admin" } },
        "integratedAuth": ["*.corp.local"],
        "profiles": "browser-profiles/"    // web-test style app profiles: panes, menus, ExtJS
    }
}
```

Secrets are named, never written: `secret` is a keychain entry the user fills in Settings, and
`fill` with `credential` types it without the model seeing it. Agents may write the section through the
normal edit path, which always asks for `drift.json`. Project config can narrow what the browser may do;
it cannot grant a secret the user has not stored.

### IE mode

IEDriverServer attached to Edge, driven over classic WebDriver as web-test does today
(`driver-manager.ts` in web-test is the reference). It cannot run hidden, so Edge's window is placed
off-screen. The pane polls WebDriver screenshots a few times a second while it is open; input goes
through WebDriver actions. Console and network are not available and the tool says so. web-test's app
layer (frame paths, VBScript, ExtJS helpers, menus) ports as data-driven profiles.

### Firefox and Camoufox

Firefox through its built-in WebDriver BiDi; the pane polls `browsingContext.captureScreenshot`.
Camoufox speaks only Playwright's Juggler protocol over a pipe, so it runs through a Playwright driver
process that Drift installs on first use and owns like the browser itself. It is the one helper process in
this design and starts only for a conversation that picks Camoufox.
