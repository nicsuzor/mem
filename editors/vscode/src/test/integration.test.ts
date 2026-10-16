import * as assert from 'assert';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';
import { spawn } from 'child_process';
import {
  createMarkdownItPlugin,
  openReferencedFile,
  PkbPreviewIndex,
} from '../extension';

// Helper to write JSON-RPC messages with Content-Length header
function writeLspMessage(stream: NodeJS.WritableStream, msg: any) {
  const json = JSON.stringify(msg);
  const header = `Content-Length: ${Buffer.byteLength(json, 'utf8')}\r\n\r\n`;
  stream.write(header + json);
}

// Helper to read JSON-RPC responses from LSP stdout stream
class LspReader {
  private buffer = Buffer.alloc(0);
  private queue: ((msg: any) => void)[] = [];
  private pending: any[] = [];

  constructor(stream: NodeJS.ReadableStream) {
    stream.on('data', (chunk: Buffer) => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
      this.parse();
    });
  }

  private parse() {
    while (true) {
      const headerEnd = this.buffer.indexOf('\r\n\r\n');
      if (headerEnd === -1) break;

      const header = this.buffer.slice(0, headerEnd).toString('utf8');
      const match = /Content-Length:\s*(\d+)/i.exec(header);
      if (!match) break;

      const contentLength = parseInt(match[1], 10);
      const totalLength = headerEnd + 4 + contentLength;
      if (this.buffer.length < totalLength) break;

      const bodyBuffer = this.buffer.slice(headerEnd + 4, totalLength);
      this.buffer = this.buffer.slice(totalLength);

      const msg = JSON.parse(bodyBuffer.toString('utf8'));
      if (this.queue.length > 0) {
        const resolve = this.queue.shift()!;
        resolve(msg);
      } else {
        this.pending.push(msg);
      }
    }
  }

  async nextMessage(): Promise<any> {
    if (this.pending.length > 0) {
      return this.pending.shift()!;
    }
    return new Promise((resolve) => {
      this.queue.push(resolve);
    });
  }
}

describe('VS Code Extension - End-to-End LSP & Markdown Preview Integration', () => {
  let tmpDir: string;
  let pkbProc: any;
  let reader: LspReader;

  before(async () => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mem-lsp-e2e-'));

    // Create a referenced task file
    const tasksDir = path.join(tmpDir, 'tasks');
    fs.mkdirSync(tasksDir, { recursive: true });
    const taskPath = path.join(tasksDir, 'task-integration.md');
    fs.writeFileSync(
      taskPath,
      `---
id: task-integration
title: E2E Integration Task
type: task
status: ready
intent: 2
tags: [integration, lsp]
---
This is the complete text of the integration task.
It must be shown in hover preview and opened in a new tab.
`
    );

    // Find pkb binary
    const pkbBin = path.resolve(__dirname, '../../../../target/debug/pkb');
    assert.ok(fs.existsSync(pkbBin), `pkb binary must exist at ${pkbBin}`);

    // Spawn pkb lsp (passing --stdio as vscode-languageclient does)
    pkbProc = spawn(pkbBin, ['lsp', '--root', tmpDir, '--stdio'], {
      stdio: ['pipe', 'pipe', 'inherit'],
    });
    reader = new LspReader(pkbProc.stdout);

    // Initialize LSP
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        rootUri: `file://${tmpDir}`,
        capabilities: {},
      },
    });

    const initResp = await reader.nextMessage();
    assert.strictEqual(initResp.id, 1);
    assert.ok(initResp.result.capabilities.hoverProvider, 'Server must advertise hover capability');
    assert.ok(initResp.result.capabilities.definitionProvider, 'Server must advertise definition capability');
    assert.ok(initResp.result.capabilities.inlayHintProvider, 'Server must advertise inlay hint capability');
    assert.ok(initResp.result.capabilities.semanticTokensProvider, 'Server must advertise semantic tokens capability');
    assert.ok(initResp.result.capabilities.codeLensProvider, 'Server must advertise code lens capability');

    // Send initialized
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      method: 'initialized',
      params: {},
    });
  });

  after(() => {
    if (pkbProc) {
      pkbProc.kill();
    }
    if (tmpDir && fs.existsSync(tmpDir)) {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  });

  it('Hovering a PKB reference in markdown shows a preview of the referenced node', async () => {
    const docUri = `file://${path.join(tmpDir, 'notes', 'referencing.md')}`;
    const docText = '# References\n\nTake a look at [[task-integration]] for details.\n';

    // 1. Open document
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      method: 'textDocument/didOpen',
      params: {
        textDocument: {
          uri: docUri,
          languageId: 'markdown',
          version: 1,
          text: docText,
        },
      },
    });

    // 2. Request hover on [[task-integration]] (line 2, char 20)
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 2,
      method: 'textDocument/hover',
      params: {
        textDocument: { uri: docUri },
        position: { line: 2, character: 20 },
      },
    });

    const hoverResp = await reader.nextMessage();
    assert.strictEqual(hoverResp.id, 2);
    assert.ok(hoverResp.result, 'Hover response must not be null');

    const hoverContents = hoverResp.result.contents;
    const hoverMarkdown = typeof hoverContents === 'string' ? hoverContents : hoverContents.value;

    assert.ok(
      hoverMarkdown.includes('E2E Integration Task'),
      `Hover preview must contain title 'E2E Integration Task', got: ${hoverMarkdown}`
    );
    assert.ok(
      hoverMarkdown.includes('task-integration'),
      `Hover preview must contain ID 'task-integration', got: ${hoverMarkdown}`
    );
    assert.ok(
      hoverMarkdown.includes('ready'),
      `Hover preview must contain status 'ready', got: ${hoverMarkdown}`
    );
    assert.ok(
      hoverMarkdown.includes('This is the complete text of the integration task'),
      `Hover preview must contain body snippet, got: ${hoverMarkdown}`
    );
  });

  it('The reference opens the whole referenced file in a new editor tab', async () => {
    const docUri = `file://${path.join(tmpDir, 'notes', 'referencing.md')}`;

    // 1. Send textDocument/definition to get the target file URI
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 3,
      method: 'textDocument/definition',
      params: {
        textDocument: { uri: docUri },
        position: { line: 2, character: 20 },
      },
    });

    const defResp = await reader.nextMessage();
    assert.strictEqual(defResp.id, 3);
    assert.ok(defResp.result, 'Definition response must not be null');

    const targetUri = defResp.result.uri || defResp.result[0]?.uri;
    assert.ok(targetUri, 'Target URI must be provided');
    assert.ok(
      targetUri.endsWith('task-integration.md'),
      `Target URI must point to task-integration.md, got: ${targetUri}`
    );

    // 2. Verify that reading the referenced target file gives the whole file content
    const targetFsPath = targetUri.replace('file://', '');
    const fullContent = fs.readFileSync(targetFsPath, 'utf8');
    assert.ok(
      fullContent.includes('This is the complete text of the integration task.'),
      'Whole referenced file must be readable from target URI'
    );
    assert.ok(
      fullContent.includes('It must be shown in hover preview and opened in a new tab.'),
      'Whole referenced file content must be intact'
    );

    // 3. Test openReferencedFile opens this file in a new editor tab (preview: false)
    const mockOpenedDocs: any[] = [];
    const mockShowOptions: any[] = [];

    const mockWorkspace: any = {
      openTextDocument: async (uri: any) => {
        mockOpenedDocs.push(uri);
        return { uri, getText: () => fullContent };
      },
    };

    const mockWindow: any = {
      showTextDocument: async (doc: any, options: any) => {
        mockShowOptions.push(options);
        return { document: doc };
      },
    };

    const editor = await openReferencedFile(
      targetUri,
      { preview: false },
      mockWorkspace,
      mockWindow
    );

    assert.ok(editor, 'Editor instance must be returned');
    assert.strictEqual(mockOpenedDocs.length, 1);
    assert.strictEqual(mockShowOptions.length, 1);
    assert.strictEqual(
      mockShowOptions[0].preview,
      false,
      'Opening reference must set preview: false to ensure a new editor tab'
    );
  });

  it('Markdown Preview window renders hover preview cards and links that open the whole file in a new tab', async () => {
    const index = new PkbPreviewIndex(tmpDir);
    const plugin = createMarkdownItPlugin(index);

    const previewHtml = plugin.render(
      'In Markdown Preview: [[task-integration]] and [Integration Link](task-integration) and bare task-integration.'
    );

    const expectedTaskPath = path.join(tmpDir, 'tasks', 'task-integration.md');
    const expectedUri = `file://${expectedTaskPath}`;

    assert.ok(
      previewHtml.includes(`href="${expectedUri}"`),
      `Markdown Preview must link to ${expectedUri}`
    );
    assert.ok(
      previewHtml.includes('mem-pkb-hover-card'),
      'Markdown Preview must include rich hover card'
    );
    assert.ok(
      previewHtml.includes('E2E Integration Task'),
      'Markdown Preview hover card must include node title'
    );
    assert.ok(
      previewHtml.includes('This is the complete text of the integration task.'),
      'Markdown Preview hover card must include node body excerpt'
    );

    // Clicking the link opens the whole file in a new editor tab (preview: false)
    const mockShowOptions: any[] = [];
    const fullContent = fs.readFileSync(expectedTaskPath, 'utf8');
    const editor = await openReferencedFile(
      expectedUri,
      { preview: false },
      {
        openTextDocument: async (uri: any) => ({ uri, getText: () => fullContent }),
      } as any,
      {
        showTextDocument: async (doc: any, options: any) => {
          mockShowOptions.push(options);
          return { document: doc };
        },
      } as any
    );

    assert.strictEqual(editor.document.getText(), fullContent);
    assert.strictEqual(mockShowOptions[0].preview, false);
  });

  it('Live LSP returns inlay hints showing target title, status, and weight', async () => {
    const docUri = `file://${path.join(tmpDir, 'notes', 'referencing.md')}`;
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 4,
      method: 'textDocument/inlayHint',
      params: {
        textDocument: { uri: docUri },
        range: {
          start: { line: 0, character: 0 },
          end: { line: 10, character: 0 },
        },
      },
    });

    const resp = await reader.nextMessage();
    assert.strictEqual(resp.id, 4);
    assert.ok(Array.isArray(resp.result), 'Inlay hints result must be an array');
    assert.strictEqual(resp.result.length, 1);
    const hint = resp.result[0];
    assert.strictEqual(hint.position.line, 2);
    assert.ok(hint.label.includes('E2E Integration Task'), `Hint label must include title, got: ${hint.label}`);
    assert.ok(hint.label.includes('ready'), `Hint label must include status 'ready', got: ${hint.label}`);
    assert.ok(hint.label.includes('w:'), `Hint label must include weight 'w:', got: ${hint.label}`);
  });

  it('Live LSP returns semantic tokens for PKB link nodes', async () => {
    const docUri = `file://${path.join(tmpDir, 'notes', 'referencing.md')}`;
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 5,
      method: 'textDocument/semanticTokens/full',
      params: {
        textDocument: { uri: docUri },
      },
    });

    const resp = await reader.nextMessage();
    assert.strictEqual(resp.id, 5);
    assert.ok(resp.result && Array.isArray(resp.result.data), 'Semantic tokens result must contain data array');
    assert.ok(resp.result.data.length >= 5, 'Must contain at least 1 encoded semantic token');
  });

  it('Live LSP returns CodeLens header bar with parent, backlinks, subtasks, and dispatch action', async () => {
    const docUri = `file://${path.join(tmpDir, 'notes', 'referencing.md')}`;
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 6,
      method: 'textDocument/codeLens',
      params: {
        textDocument: { uri: docUri },
      },
    });

    const resp = await reader.nextMessage();
    assert.strictEqual(resp.id, 6);
    assert.ok(Array.isArray(resp.result), 'CodeLens result must be an array');
    assert.ok(resp.result.length >= 2, 'Should have header bar lens and dispatch lens');
    const titles = resp.result.map((cl: any) => cl.command?.title);
    assert.ok(titles.some((t: string) => t && t.includes('Dispatch')), `Must include Dispatch lens, got: ${titles}`);
  });

  it('Live LSP returns gutter badges for lines containing task links', async () => {
    const docUri = `file://${path.join(tmpDir, 'notes', 'referencing.md')}`;
    writeLspMessage(pkbProc.stdin, {
      jsonrpc: '2.0',
      id: 7,
      method: 'mem/gutterBadges',
      params: {
        textDocument: { uri: docUri },
      },
    });

    const resp = await reader.nextMessage();
    assert.strictEqual(resp.id, 7);
    assert.ok(Array.isArray(resp.result), 'Gutter badges result must be an array');
    assert.strictEqual(resp.result.length, 1);
    const badge = resp.result[0];
    assert.strictEqual(badge.line, 2);
    assert.strictEqual(badge.status, 'ready');
    assert.strictEqual(badge.taskId || badge.task_id, 'task-integration');
  });
});
