import * as path from 'path';
import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

export interface OpenReferenceOptions {
  preview?: boolean;
  viewColumn?: vscode.ViewColumn;
}

/**
 * Open a referenced file in an editor tab.
 * Default preview: false ensures the whole file opens in a permanent new editor tab.
 */
export async function openReferencedFile(
  target: string | vscode.Uri,
  options: OpenReferenceOptions = { preview: false, viewColumn: vscode.ViewColumn.Active },
  workspace: typeof vscode.workspace = vscode.workspace,
  window: typeof vscode.window = vscode.window
): Promise<vscode.TextEditor> {
  let uri: vscode.Uri;
  if (typeof target === 'string') {
    if (target.startsWith('file://')) {
      uri = vscode.Uri.parse(target);
    } else {
      uri = vscode.Uri.file(target);
    }
  } else {
    uri = target;
  }

  const doc = await workspace.openTextDocument(uri);
  const editor = await window.showTextDocument(doc, {
    preview: options.preview ?? false,
    viewColumn: options.viewColumn ?? vscode.ViewColumn.Active,
  });
  return editor;
}

/**
 * Extract reference span at position from a line of markdown text.
 */
export function extractReferenceAtPosition(
  lineText: string,
  charIndex: number
): { target: string; alias?: string; start: number; end: number } | undefined {
  // 1. Wikilinks [[target]] or [[target|alias]]
  const wikiRegex = /\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g;
  let match: RegExpExecArray | null;
  while ((match = wikiRegex.exec(lineText)) !== null) {
    const start = match.index;
    const end = match.index + match[0].length;
    if (charIndex >= start && charIndex <= end) {
      return {
        target: match[1].trim(),
        alias: match[2]?.trim(),
        start,
        end,
      };
    }
  }

  // 2. Markdown links [label](target)
  const mdRegex = /\[([^\]]+)\]\(([^)]+)\)/g;
  while ((match = mdRegex.exec(lineText)) !== null) {
    const start = match.index;
    const end = match.index + match[0].length;
    if (charIndex >= start && charIndex <= end) {
      const target = match[2].trim();
      if (!target.startsWith('http://') && !target.startsWith('https://')) {
        return {
          target,
          alias: match[1].trim(),
          start,
          end,
        };
      }
    }
  }

  // 3. Bare PKB IDs (e.g. task_5b905e12, mem_1e3f8515, task-123)
  const wordRegex = /[a-zA-Z0-9_\-\.]+/g;
  while ((match = wordRegex.exec(lineText)) !== null) {
    let word = match[0];
    let start = match.index;
    let end = match.index + word.length;
    // Strip trailing punctuation (like '.' at end of sentence)
    while (word.endsWith('.') || word.endsWith(',') || word.endsWith(';') || word.endsWith(':')) {
      word = word.slice(0, -1);
      end -= 1;
    }
    if (charIndex >= start && charIndex <= end && word.length > 0) {
      return {
        target: word,
        start,
        end,
      };
    }
  }

  return undefined;
}

export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration('mem.lsp');
  const executable = config.get<string>('executable') || 'pkb';

  let pkbRoot = config.get<string>('pkbRoot');
  if (!pkbRoot) {
    pkbRoot =
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ||
      process.env.ACA_DATA ||
      '.';
  }

  // Server options: launch `pkb lsp --root <pkbRoot>` over stdio
  const serverOptions: ServerOptions = {
    run: {
      command: executable,
      args: ['lsp', '--root', pkbRoot],
      transport: TransportKind.stdio,
    },
    debug: {
      command: executable,
      args: ['lsp', '--root', pkbRoot],
      transport: TransportKind.stdio,
    },
  };

  // Client options: monitor markdown documents
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'markdown' },
      { scheme: 'untitled', language: 'markdown' },
    ],
    synchronize: {
      configurationSection: 'mem.lsp',
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.md'),
    },
  };

  client = new LanguageClient(
    'mem-lsp',
    'Mem PKB Language Server',
    serverOptions,
    clientOptions
  );

  client.start();

  // Register command to open referenced file in a new tab
  const openCmd = vscode.commands.registerCommand(
    'mem.openReference',
    async (target: string | vscode.Uri) => {
      return await openReferencedFile(target, {
        preview: false,
        viewColumn: vscode.ViewColumn.Active,
      });
    }
  );

  // Register command to open reference under active cursor
  const openAtCursorCmd = vscode.commands.registerCommand(
    'mem.openReferenceAtCursor',
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor || !client) {
        return;
      }

      const pos = editor.selection.active;
      const docUri = editor.document.uri.toString();

      try {
        const def: any = await client.sendRequest('textDocument/definition', {
          textDocument: { uri: docUri },
          position: { line: pos.line, character: pos.character },
        });

        if (def) {
          const targetUri = Array.isArray(def)
            ? def[0]?.uri || def[0]?.targetUri
            : def.uri || def.targetUri;
          if (targetUri) {
            await openReferencedFile(targetUri, {
              preview: false,
              viewColumn: vscode.ViewColumn.Active,
            });
          }
        }
      } catch (err) {
        vscode.window.showErrorMessage(
          `Failed to open reference: ${err instanceof Error ? err.message : String(err)}`
        );
      }
    }
  );

  // Register command to restart LSP server
  const restartCmd = vscode.commands.registerCommand(
    'mem.restartServer',
    async () => {
      if (client) {
        await client.stop();
        client.start();
        vscode.window.showInformationMessage('Mem PKB Language Server restarted.');
      }
    }
  );

  context.subscriptions.push(openCmd, openAtCursorCmd, restartCmd);
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}
