# Theming

Themes are pure CSS custom properties in `src/styles/app.css`, switched by
`data-theme` on `<html>`. The registry lives in `src/state/theme.ts`.

## Tokens

| Token | Purpose |
| --- | --- |
| `--bg` | app background |
| `--surface` | sidebar, cards, composer |
| `--raised` | hover states, code blocks, chips |
| `--overlay` | popovers/menus |
| `--edge` / `--edge-strong` | borders, dividers |
| `--ink` / `--ink-muted` / `--ink-faint` | text hierarchy |
| `--accent` / `--accent-ink` | primary actions, active states / text on accent |
| `--ok` / `--warn` / `--danger` | status colors |
| `--ui-font` / `--code-font` | interface and code font stacks |

Tailwind maps them via `@theme inline` so components use `bg-surface`, `text-ink-muted`,
`border-edge` etc. Components must never hardcode colors.

## Adding a theme

1. Add a `:root[data-theme="name"]` block defining all tokens (set `color-scheme`).
2. Add the name to `themes` in `src/state/theme.ts`.

Shipped presets: `drift-dark` (default), `drift-graphite`, `drift-midnight`,
`drift-slate`, `drift-forest`, `drift-aubergine`, `drift-light`, and `drift-paper`.
`drift-custom` derives the full token set from four persisted palette colors.

The Appearance settings also support UI and code font stacks plus local custom CSS.
Custom CSS is capped at 20 KB, persisted after a short debounce, and applied through
one style element so typing does not synchronously rewrite local storage or the DOM.

Shiki picks `github-light` for light presets or a light custom background and
`github-dark-default` otherwise (`src/ui/markdown.tsx`).
