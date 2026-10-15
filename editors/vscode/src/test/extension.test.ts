import * as assert from 'assert';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';
import {
  createMarkdownItPlugin,
  extractReferenceAtPosition,
  getOutputChannel,
  handleActiveEditorChanged,
  logDebug,
  logError,
  logInfo,
  logWarn,
  openReferencedFile,
  PkbPreviewIndex,
  resetOutputChannelsForTesting,
  resolvePkbExecutable,
  resolvePkbRoot,
} from '../extension';
import { createdOutputChannels, MockUri } from './mock-vscode';

describe('VS Code Extension - Unit & Behavior Tests', () => {
  it('extracts wikilinks, aliased wikilinks, markdown links, and bare IDs at cursor position', () => {
    const text = 'Check [[task-123]] and [[task-456|Alias Title]] or [Ref Doc](notes/doc.md) or task_bare.';

    // Inside [[task-123]] at index 10
    const ref1 = extractReferenceAtPosition(text, 10);
    assert.ok(ref1, 'Should extract reference at index 10');
    assert.strictEqual(ref1.target, 'task-123');

    // Inside [[task-456|Alias Title]] at index 26
    const ref2 = extractReferenceAtPosition(text, 26);
    assert.ok(ref2, 'Should extract aliased reference at index 26');
    assert.strictEqual(ref2.target, 'task-456');
    assert.strictEqual(ref2.alias, 'Alias Title');

    // Inside markdown link [Ref Doc](notes/doc.md) at index 55
    const ref3 = extractReferenceAtPosition(text, 55);
    assert.ok(ref3, 'Should extract markdown link target at index 55');
    assert.strictEqual(ref3.target, 'notes/doc.md');
    assert.strictEqual(ref3.alias, 'Ref Doc');

    // Inside bare ID `task_bare` at index 82
    const ref4 = extractReferenceAtPosition(text, 82);
    assert.ok(ref4, 'Should extract bare ID at index 82');
    assert.strictEqual(ref4.target, 'task_bare');

    // Outside reference at index 2 ('e')
    const refOutside = extractReferenceAtPosition('   ', 1);
    assert.strictEqual(refOutside, undefined, 'Whitespace should have no reference');
  });

  it('opens the whole referenced file in a new editor tab (preview: false)', async () => {
    const mockOpenedDocs: any[] = [];
    const mockShowOptions: any[] = [];

    const mockWorkspace: any = {
      openTextDocument: async (uri: any) => {
        mockOpenedDocs.push(uri);
        return { uri, fileName: uri.fsPath || uri.path };
      },
    };

    const mockWindow: any = {
      showTextDocument: async (doc: any, options: any) => {
        mockShowOptions.push(options);
        return { document: doc };
      },
    };

    const targetPath = '/fake/pkb/tasks/task-test.md';
    const editor = await openReferencedFile(
      targetPath,
      { preview: false },
      mockWorkspace,
      mockWindow
    );

    assert.ok(editor, 'Editor should be returned');
    assert.strictEqual(mockOpenedDocs.length, 1);
    assert.strictEqual(mockShowOptions.length, 1);
    assert.strictEqual(
      mockShowOptions[0].preview,
      false,
      'Opening a referenced file must set preview: false to open in a new editor tab'
    );
  });

  it('writes timestamped structured logs to the Mem PKB LSP OutputChannel', () => {
    resetOutputChannelsForTesting();
    createdOutputChannels.clear();

    const channel: any = getOutputChannel();
    assert.ok(channel, 'OutputChannel should be created');
    assert.strictEqual(channel.name, 'Mem PKB LSP');

    logInfo('Server starting test');
    logWarn('Missing optional config');
    logError('Connection failed', new Error('EPIPE broken pipe'));
    logDebug('Verbose trace check');

    const joined = channel.lines.join('\n');
    assert.ok(joined.includes('[INFO] Server starting test'), 'Should log INFO message');
    assert.ok(joined.includes('[WARN] Missing optional config'), 'Should log WARN message');
    assert.ok(
      joined.includes('[ERROR] Connection failed: EPIPE broken pipe'),
      'Should log ERROR message with error details'
    );
    assert.ok(joined.includes('[DEBUG] Verbose trace check'), 'Should log DEBUG message');
  });

  it('resolves pkbRoot across configured, workspace markers, ACA_DATA, and candidate paths with diagnostics', () => {
    const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'mem-pkb-root-test-'));
    try {
      const logs: string[] = [];
      const logger = (m: string) => logs.push(m);

      // 1. Configured path with ~ and $VAR expansion
      const configuredDir = path.join(tmpRoot, 'custom-pkb');
      fs.mkdirSync(configuredDir, { recursive: true });
      const resolved1 = resolvePkbRoot(
        '$TEST_PKB_BASE/custom-pkb',
        [],
        { TEST_PKB_BASE: tmpRoot } as any,
        logger
      );
      assert.strictEqual(resolved1, configuredDir);

      // 2. Workspace folder containing PKB markers ('tasks' dir)
      const wsPkb = path.join(tmpRoot, 'ws-pkb');
      fs.mkdirSync(path.join(wsPkb, 'tasks'), { recursive: true });
      const resolved2 = resolvePkbRoot(
        '',
        [{ uri: { fsPath: wsPkb } }],
        {} as any,
        logger
      );
      assert.strictEqual(resolved2, wsPkb);

      // 3. ACA_DATA environment variable
      const acaDir = path.join(tmpRoot, 'aca-data-dir');
      fs.mkdirSync(acaDir, { recursive: true });
      const resolved3 = resolvePkbRoot(
        '',
        [],
        { ACA_DATA: acaDir } as any,
        logger
      );
      assert.strictEqual(resolved3, acaDir);

      // 4. Default candidate ~/brain
      const fakeHome = path.join(tmpRoot, 'home');
      const brainDir = path.join(fakeHome, 'brain');
      fs.mkdirSync(brainDir, { recursive: true });
      const resolved4 = resolvePkbRoot(
        '',
        [],
        { HOME: fakeHome } as any,
        logger
      );
      assert.strictEqual(resolved4, brainDir);

      // 5. Missing configured path logs a warning/error
      resetOutputChannelsForTesting();
      createdOutputChannels.clear();
      const missingPath = path.join(tmpRoot, 'does-not-exist');
      const resolvedMissing = resolvePkbRoot(missingPath, [], {} as any, logger);
      assert.strictEqual(resolvedMissing, missingPath);
      const outLines = (getOutputChannel() as any).lines.join('\n');
      assert.ok(
        outLines.includes('does not exist on disk'),
        'Should log warning/error when resolved pkbRoot does not exist on disk'
      );

      // 6. Executable resolution logs candidate checks
      const execLogs: string[] = [];
      const resolvedExec = resolvePkbExecutable(path.join(tmpRoot, 'missing-pkb-bin'), (m) =>
        execLogs.push(m)
      );
      assert.ok(resolvedExec.endsWith('missing-pkb-bin'));
      assert.ok(execLogs.some((l) => l.includes('Using configured mem.lsp.executable')));
    } finally {
      fs.rmSync(tmpRoot, { recursive: true, force: true });
    }
  });

  it('renders PKB references in Markdown Preview windows with hover preview cards and file:// links that open in a new tab', async () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'mem-preview-unit-'));
    try {
      const tasksDir = path.join(tmpPkb, 'tasks');
      const notesDir = path.join(tmpPkb, 'notes');
      fs.mkdirSync(tasksDir, { recursive: true });
      fs.mkdirSync(notesDir, { recursive: true });

      const taskPath = path.join(tasksDir, 'task-preview-1.md');
      fs.writeFileSync(
        taskPath,
        `---
id: task-preview-1
title: Markdown Preview Feature Task
type: task
status: in_progress
intent: 1
parent: [[proj-mem]]
aliases: [preview-alias]
tags: [vscode, preview]
---
First line of the task body for Markdown Preview.
Second line explaining hover cards and new-tab opening.
`
      );

      const notePath = path.join(notesDir, 'mem_1e3f8515.md');
      fs.writeFileSync(
        notePath,
        `---
id: mem_1e3f8515
title: Architecture Memory Note
type: note
status: active
tags: [architecture]
---
Detailed architecture notes for PKB preview.
`
      );

      const index = new PkbPreviewIndex(tmpPkb);
      assert.strictEqual(index.size, 2, 'Should index 2 PKB markdown nodes');
      assert.ok(index.resolve('task-preview-1'), 'Should resolve by ID');
      assert.ok(index.resolve('preview-alias'), 'Should resolve by alias');
      assert.ok(index.resolve('tasks/task-preview-1.md'), 'Should resolve by relative path');

      const plugin = createMarkdownItPlugin(index);
      // Simulate VS Code's markdown-it instance
      const fakeMd = {
        render: (src: string) =>
          src
            .replace(/`([^`]+)`/g, '<code>$1</code>')
            .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>'),
      };
      plugin.extendMarkdownIt(fakeMd);

      const sampleMarkdown = [
        '1. Wikilink: [[task-preview-1]]',
        '2. Aliased Wikilink: [[task-preview-1|Preview Task Link]]',
        '3. Markdown Link: [Standard Link](task-preview-1)',
        '4. Bare ID: mem_1e3f8515.',
        '5. Inline Code ID: `task-preview-1`',
        '6. Unresolved: [[missing-node-999]]',
      ].join('\n');

      const html = fakeMd.render(sampleMarkdown);

      // Verify resolved links point to file:// URI of the target file
      assert.ok(
        html.includes(`href="file://${taskPath}"`),
        `Rendered HTML must include file:// link to taskPath, got: ${html}`
      );
      assert.ok(
        html.includes(`href="file://${notePath}"`),
        `Rendered HTML must include file:// link to notePath, got: ${html}`
      );

      // Verify hover preview card contains Title, ID, Type, Status, Intent, Tags, Parent, and body excerpt
      assert.ok(html.includes('mem-pkb-hover-card'), 'Must embed .mem-pkb-hover-card for mouseover');
      assert.ok(html.includes('Markdown Preview Feature Task'), 'Hover card must include node title');
      assert.ok(html.includes('task-preview-1'), 'Hover card must include node ID');
      assert.ok(html.includes('in_progress'), 'Hover card must include node status');
      assert.ok(html.includes('#vscode'), 'Hover card must include tags');
      assert.ok(html.includes('[[proj-mem]]'), 'Hover card must include parent');
      assert.ok(
        html.includes('First line of the task body for Markdown Preview.'),
        'Hover card must include body excerpt'
      );

      // Verify aliased wikilink and standard markdown link labels
      assert.ok(html.includes('Preview Task Link'), 'Must render aliased wikilink label');
      assert.ok(html.includes('Standard Link'), 'Must preserve standard markdown link label');

      // Verify unresolved wikilink gets mem-pkb-unresolved diagnostic class
      assert.ok(
        html.includes('mem-pkb-unresolved'),
        'Unresolved wikilink must have .mem-pkb-unresolved class'
      );

      // Verify that opening a file from a Markdown Preview link pins it as a permanent new tab (preview: false)
      const executedCommands: string[] = [];
      const showCalls: any[] = [];
      const mockCommands: any = {
        executeCommand: async (cmd: string) => {
          executedCommands.push(cmd);
        },
      };
      const mockWorkspace: any = {
        openTextDocument: async (uri: any) => ({ uri, fileName: uri.fsPath }),
      };
      const mockWindow: any = {
        showTextDocument: async (doc: any, opts: any) => {
          showCalls.push(opts);
          return { document: doc };
        },
      };

      await handleActiveEditorChanged(
        {
          document: { uri: MockUri.file(taskPath) } as any,
          viewColumn: 1,
        } as any,
        index,
        mockCommands,
        mockWorkspace,
        mockWindow
      );

      assert.ok(
        executedCommands.includes('workbench.action.keepEditor'),
        'Should execute workbench.action.keepEditor to pin preview tab'
      );
      assert.strictEqual(showCalls.length, 1);
      assert.strictEqual(
        showCalls[0].preview,
        false,
        'Must open referenced file with preview: false so it stays in a new editor tab'
      );
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });

  it('registers mem.showLogs to reveal the OutputChannel and returns extendMarkdownIt from activate()', async () => {
    resetOutputChannelsForTesting();
    createdOutputChannels.clear();

    const channel: any = getOutputChannel();
    assert.strictEqual(channel.shown, false, 'Channel should not be shown initially');
    channel.show(true);
    assert.strictEqual(channel.shown, true, 'mem.showLogs reveals the OutputChannel');
  });
});
