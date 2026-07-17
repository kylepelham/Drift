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

Tailwind maps them via `@theme inline` so components use `bg-surface`, `text-ink-muted`,
`border-edge` etc. Components must never hardcode colors.

## Adding a theme

1. Add a `:root[data-theme="name"]` block defining all tokens (set `color-scheme`).
2. Add the name to `themes` in `src/state/theme.ts`.

Shipped: `drift-dark` (default, dark grey), `drift-slate` (blue-tinted dark),
`drift-light`.

Shiki picks `github-light` for the light theme and `github-dark-default` otherwise
(`src/ui/markdown.tsx`).
