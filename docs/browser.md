# Browser

Each conversation can have one browser the agent tests in. The engine starts it, owns every process it
runs, and streams it into a browser pane beside the chat. The pane opens when the agent starts testing, or
when the user opens a browser from the globe button in the chat header; collapsing it stops the stream, not
the browser, so testing carries on unseen. Going to another conversation and back finds the pane as it was,
for as long as the browser is still running.

This replaces the chrome-devtools and web-test MCP servers for testing: no browser windows open across the
desktop, nothing outlives Drift, remote devices see the same pane, and projects that need IE mode get it.

## Engines

| Engine | Driven over | Picture for the pane | Notes |
| --- | --- | --- | --- |
| Chrome, Edge | DevTools Protocol | Screencast, sent only when the page paints | Hidden (headless) or a real window off-screen |
| Firefox | WebDriver BiDi | Screenshots about 30 times a second while watched (every 8th tick once the page is still), sent when they change | Always headless; no response bodies |
| Camoufox | WebDriver BiDi | As Firefox | Hardened Firefox for sites that block automated browsers |
| IE mode | Classic WebDriver through IEDriverServer | Screenshots every 0.8 s while watched | Edge in IE mode, window off-screen; no console or network logs |
| IE mode with a site list | IEDriverServer for IE mode tabs, DevTools Protocol through Drift's extension for Edge tabs | Each tab as its engine sends it | One Edge, routed by the machine's Enterprise Mode site list |

The engine module is `crates/drift-engine/src/browser`: `driver.rs` holds the `Driver` (a browser) and `Tab`
(a page) traits every engine implements, so the tool, the pane and the API never ask which one runs.
`chromium/`, `firefox.rs` (with `bidi.rs`) and `ie.rs` (with `webdriver.rs`) are the three drivers, and
`mixed.rs` joins the first and last into one browser for IE mode with a site list (`site_list.rs` reads the
list). They use the engine's existing `tokio-tungstenite`, `reqwest` and `image`; nothing new is linked and
nothing runs beside the browsers but IEDriverServer for IE mode.

Which engine opens, first match wins: the `engine` the agent passes to `open`; the project's `engines`
patterns for the address; the project's `engine`; Settings. Chrome and Edge stand in for each other when
one is missing, and the result says so. A conversation has one browser: opening another engine closes the
first, whether the agent named it or an `engines` pattern chose it (a pattern naming Chrome or Edge keeps
whichever of the two is running). The project's `engine` and Settings choose only for a browser not yet open.

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

An input event's answer waits until the page has handled it, and a handler that opens an alert, confirm or
prompt has not finished until the dialog closes. So each input event also ends its wait when the page reports
a dialog opening, and the rest of a click or key press is sent without waiting; the action's result then
says a dialog opened.

### Firefox and Camoufox

Started headless with `--remote-debugging-port 0` and a profile whose `user.js` turns BiDi on (Camoufox
ships with it off) and keeps the profile quiet. The port is read from what the browser prints. Pages use
the same helper script as Chrome, run in a BiDi sandbox the page cannot see; input is BiDi's
`input.performActions`. Response bodies are not available, and a phone-sized viewport keeps the desktop user
agent. Camoufox started without its Python launcher's fingerprint config names itself in its user agent
(`Camoufox/156.0.1-beta.36`), and sites that do not know the name serve their oldest pages, so the session
overrides it with the Firefox it is built on (`Firefox/156.0`) through `emulation.setUserAgentOverride`.
Pane pictures are sent at the page's own resolution; the agent's screenshots are scaled as Chrome's are.
Tabs open with `background: true` and switching never calls `browsingContext.activate`: headless, every tab
paints, and neither a foreground create nor an activate is ever answered, which held all the pane's input
behind it for the call's 60-second timeout.

### IE mode

IEDriverServer starts on a free port inside the browser's job, and a session asks it for Edge in IE mode
with the settings it needs: `ie.edgechromium`, Edge's path, an `initialBrowserUrl` on IEDriverServer itself
(Edge only creates the IE window for a page Trident renders), `ie.ignoreprocessmatch` (Edge's window belongs
to another process than the one started), no `forceShellWindowsApi`, and dialogs left for the agent to answer.
Edge's window is moved off-screen, since IE mode cannot run hidden. A window the page opens is moved off-screen
too as soon as it is found; windows are looked for every 0.8 s even when no pane watches. IEDriverServer's own
start window is left out of the tabs, since it cannot be scripted or pictured.

A watching pane gets a picture every 0.8 s, and at once when the user switches tabs or acts on the page; the
picture is taken before the windows are read again, so a switch shows in about half a second.

The page is driven by script in IE's own manner, as web-test did: a helper written for IE's oldest document
modes walks every same-site frame, so one snapshot covers a frameset and its refs reach elements in any
frame. Clicks fire mouseover, mousedown, mouseup and `click()`, which runs VBScript handlers too; `eval` with
`language: vbscript` runs VBScript through `execScript`. A window the page opens is a new tab. IEDriverServer
can drive one IE mode browser at a time, so a second conversation asking for IE mode is told to stop the
first.

### IE mode with a site list

Classic ASP apps run in IE mode but hand some pages to Edge (an MVC screen, a card payment page). On the users'
machines a group policy gives Edge an Enterprise Mode site list (`InternetExplorerIntegrationLevel` 1 and
`InternetExplorerIntegrationSiteList`) that names, per host and path, which engine opens a page, and which
session cookies the engines share (`<shared-cookie>`). IEDriverServer starts Edge with `--ie-mode-force`, which
keeps every page of every tab in IE mode, and Edge turns IE mode off altogether when it is started with
`--remote-debugging-port` or `--remote-debugging-pipe`, so neither driver alone can run such an app.

When the policy is set and the first address is one the list names (or there is none yet), `open` with
`engine: ie` starts one Edge itself, as the users' Edge runs:

- No debugging switch, so the site list routes every navigation and window and Edge shares the cookies it
  names, both ways, as it does for users. Drift never copies a cookie; IE mode's session cookies are HttpOnly
  and IEDriverServer cannot read cookies in Edge at all.
- Drift's extension (`chromium/relay/`) is loaded with `--load-extension` and connects to a loopback socket at a
  secret path. It speaks the browser end of the DevTools Protocol through `chrome.debugger`, so the Chrome
  driver drives Edge's Chromium tabs unchanged: `Target.setAutoAttach` attaches each tab once it has loaded,
  `Target.createTarget` opens a tab in a window of its own (started minimized and restored off-screen, so it
  never covers an IE mode tab, which shows nothing while covered), and events come back per tab session.
  `--silent-debugger-extension-api` keeps Edge's "is debugging this browser" bar off the pages.
- Edge refuses the debugger on a tab IE mode hosts, and says nothing when a tab moves into IE mode, so the
  extension looks at its tabs every second and reports one in IE mode. IEDriverServer then attaches to it:
  its "Edge" is a command that does nothing (`cmd /c rem`, with forward slashes since IEDriverServer drops
  backslashes), and with `ie.ignoreprocessmatch` it takes the first IE mode window in z-order, so Drift lifts
  its Edge's windows above all others for that moment. Edge is started with `--edge-skip-compat-layer-relaunch`
  so the process Drift starts is the browser. Windows the IE mode tabs open are then IEDriverServer's as usual;
  once the last one closes it lets go, and attaches again when a tab next enters IE mode.
- A new profile has no site list until Edge downloads it (about 4 s); an address loaded before that would open
  in Chromium, so the browser waits for `Default/DualEngine/SiteList-Enterprise.json`. Drift reads the same
  file to know where an address goes: the host with its port, then the host, then its parent domains, and
  within the site the longest matching path.
- The agent and the pane see one browser. Tabs of both engines are listed together; the active tab is the
  last one either engine brought forward, and the pane gets the frames of the engine that has it. An address
  of the other engine moves the tab there, as Edge would: the page opens in the other engine (through
  IEDriverServer's `window.open` for IE mode, so it keeps track of it) and the old tab closes. The first IE
  mode page is the exception: the Chromium tab itself loads it, and IEDriverServer attaches to what it became.
  A tab that left one engine as one appeared in the other within 10 s is taken to be the same tab, so an
  action's result follows a click that moved it.
- Every 250 ms any window of the Edge that came onto a screen is moved off it.

A Chromium tab that a click sends into IE mode while IEDriverServer already drives other IE mode tabs is not
reachable: IEDriverServer only learns of windows its own pages open. None of the apps on the dev drive do this;
their hand-offs open Edge pages from IE mode (`window.open` of `/Edge/...`, `PciPal.asp`,
`PaymentIQCashier.asp`, `ServerPayment.asp`), and those pages report back through a shared cookie the IE mode
page polls. Traces and Lighthouse need a debugging port, so they are not available in this browser.

## Lifecycle

- A conversation's browser starts the first time its agent calls `open` (or `navigate`), or when the user
  presses the globe button in the chat header (at the project's `baseUrl` when it has one). Subagents are
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
  engines/                                 Camoufox and IEDriverServer when Drift downloaded them, and Lighthouse
```

- **Isolated is the default.** A conversation starts signed out and leaves nothing behind.
- **Shared** keeps sign-ins for a workspace across its conversations, chosen in Settings or by the project.
  Two conversations of a workspace cannot open one profile at once, so the second gets its own and is told.
- Chrome's caches are capped (`--disk-cache-size` 50 MB) and its background downloads (component and
  optimization model updates, Safe Browsing lists, sync) are off; Firefox's disk cache is capped at 50 MB.
- When a shared profile's browser ends, its caches are deleted; cookies, local storage and IndexedDB stay.
- At startup, isolated profiles with no running browser (left by a crash) are deleted, as are shared profiles
  of workspaces Drift no longer has.
- IE mode's profile belongs to IEDriverServer, which makes and removes it. IE mode with a site list starts its
  own Edge on an isolated profile, which also holds Drift's extension.

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
| `screenshot` | A JPEG of the viewport for the model to look at, scaled down in its own shape to at most 1280x800's worth of pixels. |
| `zoom` | `region` `[x0, y0, x1, y1]` of what a screenshot shows now, cut from the full-resolution picture and enlarged up to 4x within the same budget. Coordinates stay the screenshot's. |
| `eval` | JavaScript in the page (VBScript in IE mode with `language: vbscript`); the result as JSON. |
| `logs` | Console messages or network requests, optionally matching `search`; `request` with an id returns its body (Chrome and Edge). |
| `dialog` | Accepts or dismisses an open alert, confirm or prompt, with optional `text`. |
| `viewport` | `desktop` (the browser's own), `tablet` (768x1024), `mobile` (390x844, touch and a phone's user agent on Chrome and Edge), or `width`/`height`; the page reloads. Every size keeps the browser's own pixel ratio: with any other, Chromium redraws the page scaled for each screenshot, and the pane's screencast showed those frames zoomed and cropped. |
| `trace` | A performance trace and its summary (Chrome and Edge): `record` is `reload` (the default), or `start` then `stop` around what the model does; `throttle` slows the CPU and network. |
| `audit` | A Lighthouse audit (Chrome and Edge): scores and failing audits; `mode` `navigation` or `snapshot`, `device` `desktop` or `mobile`. |
| `pages`, `select`, `close` | Lists tabs (popups included), makes one active, closes one. |
| `login` | Signs in as the project's `browser.login` says, with a stored credential. |
| `batch` | Runs `actions`, a list of the above, in order in one call, stopping at the first failure. |
| `stop` | Ends this conversation's browser. |

Results say what changed (the page's title and address after an action that navigated, a dialog that
opened, a popup that appeared) so the model does not need to snapshot after every click.

Screenshots have a fixed pixel budget because newer Claude models bill an image by its size (about
`width / 28 x height / 28` tokens) instead of shrinking it, and a page laid out to fill a tall pane would
otherwise cost two or three times a desktop screenshot. 1280x800 is the size Anthropic suggests for web apps,
so every screenshot costs about what one of those does, about 1,330 tokens. Clicks at screenshot points are
scaled back to CSS pixels by the last screenshot or zoom. Small text that the scaling makes hard to read is
what `zoom` is for, as in Anthropic's own browser and computer toolsets. The pane's stream is not affected.

Permission: acting on a page asks once per host (`browser` permission, the host as its pattern). Loopback,
private addresses and intranet names (one label, `.local`, `.internal`, `.lan`, `.test`, `.home.arpa`) are
allowed without asking. A path resolves against the project's base before its host is judged. An "always"
answer is kept for the workspace.

## Performance traces

`trace` records with the DevTools Protocol's `Tracing` domain, using the categories DevTools' Performance
panel records, minus screenshots. The browser writes the trace to a gzipped stream, which is read in chunks
and saved as `tool-output/<session>/<call>.trace.json.gz` (cleared with the session's other tool output after a
week). The user can load it in DevTools' Performance panel or Perfetto.

The model gets a summary, worked out in the engine from the trace events, rather than the trace:

- Load: time to first byte, first and largest contentful paint (the element, and for an image its address and
  how long it took to arrive), DOMContentLoaded and load.
- Layout shift: the worst session window, as Web Vitals counts it, and what shifted.
- Main thread: busy time by kind of work, long tasks named by what ran in them, total blocking time after
  first paint, and script time by file.
- Network: requests, bytes, failures, render-blocking requests, the slowest and largest, other origins.
- Interactions: each click or key press with its input delay, handler time and time to the next paint.

`reload` keeps recording for 3 s after the load, for late paints and shifts. `throttle` sets
`Emulation.setCPUThrottlingRate` to 4 and `Network.emulateNetworkConditions` to slow 4G (150 ms, 1.6 Mbit/s
down, 750 kbit/s up), and clears both when the trace stops.

Known issue: on a PC whose CPU cores disagree on their time stamp counters (a firmware fault; Windows itself
works around it), Chrome's tracing library fails a clock check the first time a process traces and crashes
that process: the browser's ends the browser, a page's leaves the page unresponsive. It hits at most the first
trace of each process. The PC Drift is developed on has it (its core 0 runs 2.3 s ahead); the cure is firmware or a cold boot, not
Drift.

## Lighthouse

`audit` runs Lighthouse against the conversation's own browser, connected to its DevTools port, so it audits
the page with its sign-ins and state. Lighthouse is a Node program, so it needs Node.js on PATH; the first audit
installs it with npm into `browsers/engines/lighthouse` (Lighthouse 13 on Node 22.19 or later, otherwise 12,
which lacks only the agentic browsing category). `lighthouse.mjs`, embedded in the engine, finds the page by
its target id, runs a navigation or snapshot audit and writes the HTML and JSON reports to the session's tool
output.

- Categories: accessibility, best practices and SEO (and agentic browsing on 13). Performance is left to
  `trace`, which measures the real page rather than a simulation.
- The page's storage is kept and throttling is left as it is, so the audit sees the page the user sees; the
  hidden browser's desktop user agent replaces Lighthouse's headless one.
- The model gets the scores and each failing audit with up to three of its elements; the HTML report is
  for the user.

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
  Firefox's trusted-URI preferences. IE mode follows Windows' own zones. Each entry is a host or `*.` and a
  domain of two labels or more; `*`, `*.local` and anything else are ignored with a note, so a cloned repo cannot
  hand the user's Windows sign-in to every site.
- `notes` is told to the agent when the browser starts: the place to keep what it learns about an app, as
  web-test's project notes were.

## Credentials

Settings > Browser lists the current workspace's stored sign-ins: a name, a user, a domain and whether a
password is stored. Names, users and domains are a setting per workspace; passwords are keychain secrets
(`browser:<workspace>:<name>`). The engine fills them in itself, through `login` or `fill` with `secret`, so a
password never reaches the model. Every tool result has the workspace's passwords replaced with
`[password]`, since a sign-in address or a page can echo one back. A `login` call asks for the hosts of
`login.url` and `login.path` as well as the base and current page, and a `login.url` whose host would come from
the credential is refused.

## API

- `GET /sessions/{id}/browser` the conversation's browser, or `null`.
- `POST /sessions/{id}/browser` starts it for the user (a running one is returned as it is).
- `POST /sessions/{id}/browser/stop` ends it.
- `GET /sessions/{id}/browser/view` a WebSocket. The engine sends binary messages, each a 4-byte
  little-endian header length, a JSON header (`page`, `width`, `height`) and a JPEG, and a text message
  `{"type":"cursor","cursor":...}` when the cursor under the pane's pointer changes. The client sends JSON
  text: `mouse`, `key`, `text`, `select`, `close`, `newPage`, `navigate`, `dialog`, `fit`. Frames flow only
  while at least one view socket is open, and only for the active page. Input is applied in the order it was
  sent; pointer moves and wheel turns that queue while the browser is busy fold into the last move and one summed
  turn. Firefox answers each `input.performActions` before the next can start, so without this a high-refresh
  pointer (about 200 moves a second) left Camoufox seconds behind under load; Chrome folds them itself.
- `GET /browser/engines` each engine's status, program and download progress;
  `POST /browser/engines/{engine}/download`; `PUT /browser/engines/{engine}/path`.
- `GET /workspaces/{id}/browser` the workspace's drift.json section and its credentials;
  `PUT` and `DELETE /workspaces/{id}/browser/credentials/{name}`.
- Event `browser.updated` carries the whole record (state, engine, storage, pages with their address, title,
  dialog and which is active).
- Settings: `browser` in `/settings` (the tool on or off, default engine, profile, window).

## The pane

- Opens by itself when a conversation's browser starts, unless the user collapsed it for that browser.
- The globe button in the chat header opens a browser when the conversation has none, then shows or hides
  the pane.
- The engine's name, tabs for the browser's pages (popups included), an address bar with back, forward and
  reload, a new-tab button, and Stop; the collapse button only hides the pane. Tabs take the room the header
  has, narrow to 80 px before the strip scrolls, and scroll with the mouse wheel.
- A dialog on the page shows as a bar with its message, a field for a prompt, and OK and Cancel.
- Until a picture of the active tab arrives (a tab switch in IE mode takes about half a second), the old
  picture dims under a sliding bar; it shows only after 150 ms, so a quick switch does not flicker.
- The page is laid out to fill the pane (Chrome, Edge, Firefox and Camoufox): the pane sends the size of its
  page area (`fit`,
  again 150 ms after each resize settles), and the page gets a viewport at least 1280 CSS pixels wide in the
  pane's shape (height over width kept between 0.4 and 2.5), so pages keep their desktop layout and a pane
  narrower than that shows it scaled down. A size the agent chose with `viewport` wins until it asks for
  `desktop` again; the picture is then centred in the pane. IE mode keeps its own size.
- Clicks, wheel and keys typed while the picture has focus go to the page, mapped to its CSS pixels. Pasting
  sends the text as typed input.
- The pane shows the cursor the page would (all but IE mode). CSS cursors are not reported by the DevTools
  Protocol or BiDi, so after each pointer event the engine asks the page's isolated world what is under the point:
  the element's computed `cursor` (an image cursor gives its keyword fallback), or for `auto` an I-beam over
  editable fields and selectable text and an arrow elsewhere. Lookups run one at a time on the newest
  position, and only a change is sent.
- Only the rightmost panel shows the window controls; on a phone the pane covers the chat, and a review or
  debug panel the user opened wins over it.

## What web-test had that this does not

web-test's per-app helpers (`app_menu`, `app_search`, `app_open_row`, the ExtJS operations) were code for
particular apps. Here the generic snapshot reaches into frames, refs work across them, and `eval` reaches
ExtJS and VBScript; what an app needs is kept in its project `notes`. chrome-devtools' performance traces
and Lighthouse audits are the `trace` and `audit` actions.
