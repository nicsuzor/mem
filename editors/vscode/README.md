# Mem PKB VS Code Extension

VS Code extension for the Personal Knowledge Base (PKB) backed by `pkb lsp`, supporting both the Markdown text editor and **VS Code Markdown Preview windows**.

## Features

- **Mouseover Preview (Editor & Markdown Preview Windows)**:
  - Hovering over a PKB reference in the Markdown editor or in a **Markdown Preview window** (`[[id]]`, `[[id|alias]]`, `[label](target)`, or bare task/node ID such as `task-123` or `mem_1e3f8515`) displays a rich preview card of the referenced node (Title, ID, Type, Status, Intent, Tags, Parent, and 12-line body excerpt).
  - Unresolved wikilinks (`[[missing-id]]`) are highlighted in Markdown Preview with a diagnostic warning card showing the unresolved target and active `pkbRoot`.
- **Open in New Tab (Editor & Markdown Preview Windows)**:
  - Clicking on a reference in a **Markdown Preview window** opens the whole referenced file in a permanent new editor tab (`preview: false`).
  - Clicking on a reference via Go to Definition (`F12`, `Ctrl+Click` / `Cmd+Click`) or document links in the Markdown editor opens the whole referenced file in a new editor tab.
  - Clicking `[Open in editor tab](...)` directly from the LSP hover preview opens the target file.
  - Invoking `Mem: Open Reference at Cursor in New Tab` from the command palette opens the referenced node under the cursor.
- **Debugging & Error Logging**:
  - Dedicated **`Mem PKB LSP`** and **`Mem PKB LSP Trace`** OutputChannels with ISO-8601 timestamped structured logs (`[INFO]`, `[WARN]`, `[ERROR]`, `[DEBUG]`).
  - Detailed startup diagnostics logging candidate `pkb` binary resolution and `pkbRoot` discovery (`mem.lsp.pkbRoot`, workspace PKB markers, `$ACA_DATA`, `~/brain`, `~/aca-data`).
  - Status bar indicator showing live server state (`Starting`, `Running`, or `Error`) with one-click access to status diagnostics and logs.

## Requirements

Requires the `pkb` CLI binary built with LSP support (`cargo install --path .` from the `mem` repository root).

## Extension Settings

- `mem.lsp.executable`: Path to the `pkb` executable (default: `"pkb"`).
- `mem.lsp.pkbRoot`: Path to the PKB root directory (defaults to workspace folder containing PKB markers, `$ACA_DATA`, or `~/brain`). Supports `~` and `$VAR` expansion.
- `mem.lsp.debug`: Enable verbose debug logging in the `Mem PKB LSP` OutputChannel (default: `false`).
- `mem.lsp.trace.server`: Language server communication trace level (`"off"`, `"messages"`, `"verbose"`).

## Commands

- `mem.openReference`: Open a specific PKB reference URI, path, or ID in a permanent editor tab.
- `mem.openReferenceAtCursor`: Open the referenced node under the cursor in a new editor tab.
- `mem.restartServer`: Restart the `pkb lsp` language server process and reload the Markdown Preview index.
- `mem.showVersion`: Display extension version, server status, binary version & path, resolved `pkbRoot`, indexed node count, and last error.
- `mem.showLogs`: Reveal the `Mem PKB LSP` OutputChannel logs.
