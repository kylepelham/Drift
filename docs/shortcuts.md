# Keyboard shortcuts

Everything in Drift can be done from the keyboard. Settings > Shortcuts lists every shortcut, lets the rebindable
ones be changed, and shows the fixed keys that belong to one place (the message box, the sidebar, a highlighted
message). The command palette (Ctrl+K) lists every action with its shortcut, so one with none is still a few keys
away. F1 opens the Shortcuts page.

## Choosing the defaults

- Nothing takes a key a text field uses. Ctrl+Up, Ctrl+Shift+arrows, Home and End move or select in the message
  box, so message navigation is on Alt and the arrows, which do nothing in a Windows text field.
- Familiar meanings win: Ctrl+Tab and Ctrl+1..9 move between sessions as browsers move between tabs (9 is always the
  last); Alt+Left and Alt+Right go back and forward through sessions you viewed; Ctrl+B hides the sidebar and
  Ctrl+Shift+E focuses it, as in VS Code; Ctrl+/ chooses the model, as in Cursor; F2 renames.
- Workspaces step with Ctrl+Alt+Up and Down, beside the computer switcher's Ctrl+Alt+Left and Right. Arrows, because
  AltGr is Ctrl+Alt on Windows and types brackets on many layouts.
- Ctrl+Shift+F opens session search; pressed again in it, it switches between names and content.

## How a key is handled

`src/state/keybinds.ts` holds the rebindable actions and their defaults. One listener on the window runs last, after
any field, menu or list that handled the key itself (they call `preventDefault`), so a shortcut never steals a key
from the thing that has focus. While a dialog is open only the palette and zoom keys apply. A view that takes every
key, the browser pane's canvas (`data-own-keys`), keeps them, except what a real browser keeps for itself: new
session, next and previous session, and focus the message box.

Each action has one handler, registered by the component that owns it with `onKeybind`, which returns its removal.
Fixed keys (Ctrl+digit, the sidebar's arrows, the composer's Escape) are handled where they apply.

## Places

- Message box: Enter sends, Shift+Enter breaks the line, Alt+Enter queues, Tab cycles agents, Up and Down recall
  earlier prompts. Escape first lets go of a highlighted message, then stops the running turn. Page Up and Page Down
  scroll the conversation while the draft fits without scrolling. Typing while nothing editable has focus starts a
  message.
- Messages: Alt+Up and Alt+Down ring the previous or next message, starting from the one in view; with Shift they move
  by your prompts; Alt+Home and Alt+End go to the first and latest. Ctrl+C copies the ringed message when no text is
  selected; Escape lets it go.
- Sidebar: rows take focus one at a time (the open session is the Tab stop). Up and Down move through workspaces and
  sessions, Left goes up to the workspace or folds it, Right opens it, Enter opens a session, F2 renames, Delete
  archives, Shift+F10 or the menu key opens its menu. Menus move with the arrows and give focus back when they close.
- Session search: Up and Down move through results, Enter opens one and keeps the search, Escape closes it and goes
  back to the message box.
- Pickers open with their shortcut (model Ctrl+/, thinking Ctrl+., agent unbound), start on the current choice, and
  give focus back where it was.
