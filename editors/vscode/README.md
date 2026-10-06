# Mem PKB VS Code Extension

VS Code extension for the Personal Knowledge Base (PKB) backed by `pkb lsp`.

## Features

- **Mouseover Preview**: Hovering over or highlighting a PKB reference in markdown (`[[id]]`, `[[id|alias]]`, `[label](target)`, or bare task/node ID) shows a rich markdown preview of the referenced node (Title, ID, Type, Status, Intent, Tags, Parent, and body excerpt).
- **Open in New Tab**:
  - Clicking on a reference via Go to Definition (`F12`, `Ctrl+Click` / `Cmd+Click`) opens the whole referenced file in a new editor tab.
  - Clicking document links (`Ctrl+Click` / `Cmd+Click`) in markdown directly opens the referenced file in a new editor tab.
  - Clicking `[Open in editor tab](...)` directly from the hover preview opens the target file.
  - Invoking `Mem: Open Reference at Cursor in New Tab` from the command palette opens the referenced node under the cursor.

## Requirements

Requires the `pkb` CLI binary built with LSP support (`cargo install --path .` from the `mem` repository root).

## Extension Settings

- `mem.lsp.executable`: Path to the `pkb` executable (default: `"pkb"`).
- `mem.lsp.pkbRoot`: Path to the PKB root directory (defaults to workspace folder or `$ACA_DATA`).
- `mem.lsp.trace.server`: Language server communication trace level (`"off"`, `"messages"`, `"verbose"`).

## Commands

- `mem.openReference`: Open a specific PKB reference URI or path in a permanent editor tab.
- `mem.openReferenceAtCursor`: Open the referenced node under the cursor in a new editor tab.
- `mem.restartServer`: Restart the `pkb lsp` language server process.
