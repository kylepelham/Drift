# Browser

Each conversation can have one browser the agent tests in. The engine starts it, owns every process it
runs, and streams it into a browser pane beside the chat. The pane opens when the agent starts testing;
collapsing it stops the stream, not the browser, so testing carries on unseen. Going to another
conversation and back finds the pane as it was, for as long as the browser is still running.

This replaces the chrome-devtools and web-test MCP servers for testing: no browser windows open across the
desktop, nothing outlives Drift, remote devices see the same pane, and projects that need IE mode get it.

## Engines

| Engine | Driven over | Picture for the pane | Notes |
| --- | --- | --- | --- |
| Chrome, Edge | DevTools Protocol | Screencast, sent only when the page paints | Hidden (headless) or a real window off-screen |
| Firefox | WebDriver BiDi | Screenshots every 0.5 s while watched, sent when they change | Always headless; no response bodies |
| Camoufox | WebDriver BiDi | As Firefox | Hardened Firefox for sites that block automated browsers |
| IE mode | Classic WebDriver through IEDriverServer | Screenshots every 0.8 s while watched | Edge in IE mode, window off-screen; no console or network logs |

The engine module is `crates/drift-engine/src/browser`: `driver.rs` holds the `Driver` (a browser) and `Tab`
(a page) traits every engine implements, so the tool, the pane and the API never ask which one runs.
`chromium/`, `firefox.rs` (with `bidi.rs`) and `ie.rs` (with `webdriver.rs`) are the three drivers. They
use the engine's existing `tokio-tungstenite`, `reqwest` and `image`; nothing new is linked and nothing
runs beside the browsers but IEDriverServer for IE mode.

Which engine opens, first match wins: the `engine` the agent passes to `open`; the project's `engines`
patterns for the address; the project's `engine`; Settings. Chrome and Edge stand in for each other when
one is missing, and the result says so. A conversation has one browser: opening another engine closes the
first.

Where each program comes from is in Settings > Browser > Engines:

- Chrome, Edge and Firefox are found where their installers put them, or on PATH.
- Camoufox is downloaded by Drift when the user asks (about 1.3 GB, the latest non-pre-release Windows
  build from `daijro/camoufox`), into `browsers/engines/camoufox`.
- IEDriverServer is looked for at the user's path, Drift's download, `C:\selenium\IEDriverServer.exe` and
  PATH; when none is found it is downloaded (the 32-bit build, which types quickly into IE) as soon as IE
  mode is needed.
- Every engine's program can be set by hand; for IE mode that path is IEDriverServer's.

### Chrome and Edge

The real installed browser over its DevTools Protocol, launched with `--remote-debugging-port=0` and the
port read from `DevToolsActivePort`. Pages are found by auto-attaching to every tab, popups included.
Snapshots and element lookups run in an isolated world the page's scripts cannot see. Clicks and typing
are `Input.dispatch*` events, so they are trusted. Chrome does not report a title a page's script sets, so
titles are read again after each load, after each action, and every 2 s while a pane watches. A screencast
sends nothing until the page paints, so a new watcher first gets a screenshot.

### Firefox and Camoufox

Started headless with `--remote-debugging-port 0` and a profile whose `user.js` turns BiDi on (Camoufox
ships with it off) and keeps the profile quiet. The port is read from what the browser prints. Pages use
the same helper script as Chrome, run in a BiDi sandbox the page cannot see; input is BiDi's
`input.performActions`. Response bodies are not available, and a phone-sized viewport keeps the desktop user
agent.

### IE mode

IEDriverServer starts on a free port inside the browser's job, and a session asks it for Edge in IE mode
with the settings it needs: `ie.edgechromium`, Edge's path, an `initialBrowserUrl` on IEDriverServer itself
(Edge only creates the IE window for a page Trident renders), `ie.ignoreprocessmatch` (Edge's window belongs
to another process than the one started), no `forceShellWindowsApi`, and dialogs left for the agent to answer.
Edge's window is moved off-screen, since IE mode cannot run hidden.

The page is driven by script in IE's own manner, as web-test did: a helper written for IE's oldest document
modes walks every same-site frame, so one snapshot covers a frameset and its refs reach elements in any
frame. Clicks fire mouseover, mousedown, mouseup and `click()`, which runs VBScript handlers too; `eval` with
`language: vbscript` runs VBScript through `execScript`. A window the page opens is a new tab. IEDriverServer
can drive one IE mode browser at a time, so a second conversation asking for IE mode is told to stop the
first.

## Lifecycle

- A conversation's browser starts the first time its agent calls `open` (or `navigate`). Subagents are
  conversations too: each gets its own, which ends with its run.
- It keeps running after the turn ends, after Stop, while the pane is collapsed, and while the user looks at
  another conversation.
- It ends when the agent calls `stop`, the user presses Stop in the pane, the conversation is archived or
  deleted, Drift quits, or it has been idle for 30 minutes (no tool call and no pane watching).
- The browser, its driver and everything they start live in one job object on Windows (a process group
  elsewhere), as background shells do, so a crash of Drift cannot leave one behind.
- A browser that exits on its own is reported as stopped; the next `open` starts a fresh one.
- No client hears of a browser before the API can return it, and a browser still ending is waited out
  before its replacement starts, so the new one never starts on a profile folder being deleted.

## Profiles and disk

Profiles live under Drift's data folder, named for what they are:

```text
browsers/
  sessions/<conversation id>/<engine>/     isolated: one per conversation, deleted when its browser ends
  shared/<workspace name>-<id>/<engine>/   shared: one per workspace and engine, kept between conversations
  engines/                                 Camoufox and IEDriverServer, when Drift downloaded them
```

- **Isolated is the default.** A conversation starts signed out and leaves nothing behind.
- **Shared** keeps sign-ins for a workspace across its conversations, chosen in Settings or by the project.
  Two conversations of a workspace cannot open one profile at once, so the second gets its own and is told.
- Chrome's caches are capped (`--disk-cache-size` 50 MB) and its background downloads (component and
  optimization model updates, Safe Browsing lists, sync) are off; Firefox's disk cache is capped at 50 MB.
- When a shared profile's browser ends, its caches are deleted; cookies, local storage and IndexedDB stay.
- At startup, isolated profiles with no running browser (left by a crash) are deleted, as are shared profiles
  of workspaces Drift no longer has.
- IE mode's profile belongs to IEDriverServer, which makes and removes it.

## Getting blocked less

Ordinary sites should treat the browser as a person's, without giving up a feature for it. What cannot be
done without losing a feature is left to Camoufox.

- The real installed Chrome or Edge is used, never Chromium or Chrome for Testing.
- No `--enable-automation`, and `--disable-blink-features=AutomationControlled`, so `navigator.webdriver`
  is false and no automation banner exists.
- In hidden mode the user agent and client hints are the real browser's, without `HeadlessChrome`, on every
  page and popup before it runs a script. The brand list follows Chromium's own ordering for the version.
- Clicks, typing and scrolling are real input events, with real coordinates.
- Snapshots and ref lookups run where the page's scripts cannot see them.
- The off-screen window mode runs a normal window with the real GPU, for sites that check rendering; it
  shows in the taskbar.
- Console capture needs `Runtime.enable`, which a few anti-bot scripts detect. It stays on; Camoufox is the
  answer for such sites.

## The `browser` tool

One tool with an `action`, so the schema stays small. Every action except `open`, `stop` and `pages` acts on
the active page unless `page` names another.

| Action | Does |
| --- | --- |
| `open` | Starts the browser if needed and opens `url` in a new tab (or the first blank one). A path opens on the project's `baseUrl`; no address opens the base. `engine` picks another browser. When the browser starts, the project's notes are in the result. |
| `navigate` | Loads `url` in the page, or `back`, `forward`, `reload`. |
| `snapshot` | The page as an accessibility-style tree, each element it can act on tagged `[ref=N]`. Password, card and one-time-code values are never shown. |
| `click`, `hover` | By `ref`, or by `x`/`y` in the last screenshot's pixels. `double` for a double click. |
| `fill` | Sets a field by `ref`: text into an input or editable element, an option of a select by value or label, a checkbox or radio to `checked`. `secret` types a part of a stored credential instead. |
| `type`, `press` | Text into the focused element; a key or chord such as `Enter`, `Control+A`. |
| `scroll` | By `ref` into view, or `dy` pixels. |
| `wait` | Until `text` appears or `ms` pass, at most 30 s. |
| `screenshot` | A JPEG of the viewport for the model to look at. |
| `eval` | JavaScript in the page (VBScript in IE mode with `language: vbscript`); the result as JSON. |
| `logs` | Console messages or network requests, optionally matching `search`; `request` with an id returns its body (Chrome and Edge). |
| `dialog` | Accepts or dismisses an open alert, confirm or prompt, with optional `text`. |
| `viewport` | `desktop` (the browser's own), `tablet` (768x1024), `mobile` (390x844, touch and a phone's user agent on Chrome and Edge), or `width`/`height`; the page reloads. |
| `pages`, `select`, `close` | Lists tabs (popups included), makes one active, closes one. |
| `login` | Signs in as the project's `browser.login` says, with a stored credential. |
| `batch` | Runs `actions`, a list of the above, in order in one call, stopping at the first failure. |
| `stop` | Ends this conversation's browser. |

Results say what changed (the page's title and address after an action that navigated, a dialog that
opened, a popup that appeared) so the model does not need to snapshot after every click.

Permission: acting on a page asks once per host (`browser` permission, the host as its pattern). Loopback,
private addresses and intranet names (one label, `.local`, `.internal`, `.lan`, `.test`, `.home.arpa`) are
allowed without asking. A path resolves against the project's base before its host is judged. An "always"
answer is kept for the workspace.

## Project config

A project describes its app in the `browser` section of its `drift.json`; the nearest file that sets it
wins. Agents may write it through the normal edit path, which always asks for `drift.json`.

```jsonc
{
    "browser": {
        "engine": "ie",                          // chrome | edge | firefox | camoufox | ie
        "baseUrl": "https://nt.traveller.local", // where `open` starts and paths resolve
        "engines": { "edge": ["*/Mvc/*"] },      // address patterns that pick an engine
        "storage": "shared",                     // isolated | shared
        "window": "offscreen",                   // hidden | offscreen (Chrome and Edge)
        "integratedAuth": ["*.corp.local"],      // hosts Windows signs in to with the user's account
        "login": {
            "credential": "admin",               // a credential stored for the workspace
            "path": "login.asp",                 // or "url": "main.asp?Login={user}&Pass={password}&Site={domain}"
            "fields": { "#User": "{user}", "#Pass": "{password}" },
            "submit": "#SignIn"
        },
        "notes": "Menus live in the top frame; results load into the Result frame."
    }
}
```

- `integratedAuth` becomes Chrome's `--auth-server-allowlist` and `--auth-negotiate-delegate-allowlist`, and
  Firefox's trusted-URI preferences. IE mode follows Windows' own zones.
- `notes` is told to the agent when the browser starts: the place to keep what it learns about an app, as
  web-test's project notes were.

## Credentials

Settings > Browser lists the current workspace's stored sign-ins: a name, a user, a domain and whether a
password is stored. Names, users and domains are a setting per workspace; passwords are keychain secrets
(`browser:<workspace>:<name>`). The engine fills them in itself, through `login` or `fill` with `secret`, so a
password never reaches the model. Every tool result has the workspace's passwords replaced with
`[password]`, since a sign-in address or a page can echo one back.

## API

- `GET /sessions/{id}/browser` the conversation's browser, or `null`.
- `POST /sessions/{id}/browser/stop` ends it.
- `GET /sessions/{id}/browser/view` a WebSocket. The engine sends binary messages, each a 4-byte
  little-endian header length, a JSON header (`page`, `width`, `height`) and a JPEG. The client sends JSON
  text: `mouse`, `key`, `text`, `select`, `close`, `newPage`, `navigate`, `dialog`. Frames flow only while at
  least one view socket is open, and only for the active page. Input is applied in the order it was sent.
- `GET /browser/engines` each engine's status, program and download progress;
  `POST /browser/engines/{engine}/download`; `PUT /browser/engines/{engine}/path`.
- `GET /workspaces/{id}/browser` the workspace's drift.json section and its credentials;
  `PUT` and `DELETE /workspaces/{id}/browser/credentials/{name}`.
- Event `browser.updated` carries the whole record (state, engine, storage, pages with their address, title,
  dialog and which is active).
- Settings: `browser` in `/settings` (the tool on or off, default engine, profile, window).

## The pane

- Opens by itself when a conversation's browser starts, unless the user collapsed it for that browser.
- The engine's name, tabs for the browser's pages (popups included), an address bar with back, forward and
  reload, a new-tab button, and Stop; the collapse button only hides the pane.
- A dialog on the page shows as a bar with its message, a field for a prompt, and OK and Cancel.
- The page image keeps its aspect ratio at the top of the pane; clicks, wheel and keys typed while it has
  focus go to the page, mapped to its CSS pixels. Pasting sends the text as typed input.
- A collapsed browser shows as a button in the chat header while it runs.
- Only the rightmost panel shows the window controls; on a phone the pane covers the chat, and a review or
  debug panel the user opened wins over it.

## What web-test had that this does not

web-test's per-app helpers (`app_menu`, `app_search`, `app_open_row`, the ExtJS operations) were code for
particular apps. Here the generic snapshot reaches into frames, refs work across them, and `eval` reaches
ExtJS and VBScript; what an app needs is kept in its project `notes`. Lighthouse and performance traces
remain with the chrome-devtools MCP.
