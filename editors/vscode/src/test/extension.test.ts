import * as assert from 'assert';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';
import {
  computeGutterBadges,
  computeHeaderCodeLenses,
  computeInlayHints,
  computeSemanticTokens,
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

  it('resolves pkb from PATH before /usr/local/bin/pkb, while an explicit user setting still wins', () => {
    const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'mem-pkb-exec-test-'));
    const origPath = process.env.PATH;
    const origHome = process.env.HOME;
    try {
      const pathDir = path.join(tmpRoot, 'opt-nic-bin');
      const usrLocalBin = path.join(tmpRoot, 'usr-local-bin');
      const customBinDir = path.join(tmpRoot, 'custom-bin');
      const emptyHome = path.join(tmpRoot, 'empty-home');
      fs.mkdirSync(pathDir, { recursive: true });
      fs.mkdirSync(usrLocalBin, { recursive: true });
      fs.mkdirSync(customBinDir, { recursive: true });
      fs.mkdirSync(emptyHome, { recursive: true });

      const newerPathPkb = path.join(pathDir, 'pkb');
      const explicitPkb = path.join(customBinDir, 'pkb-custom');
      fs.writeFileSync(newerPathPkb, '#!/bin/sh\necho "pkb 0.3.104"\n', { mode: 0o755 });
      fs.writeFileSync(explicitPkb, '#!/bin/sh\necho "pkb custom"\n', { mode: 0o755 });

      // Ensure /usr/local/bin/pkb also exists on disk (as in the container/host environment)
      assert.ok(
        fs.existsSync('/usr/local/bin/pkb'),
        'Precondition: /usr/local/bin/pkb exists on disk'
      );

      process.env.HOME = emptyHome;
      process.env.PATH = `${pathDir}${path.delimiter}/usr/local/bin${path.delimiter}${origPath || ''}`;

      // Case 1: Default setting ('pkb' or '') resolves the first pkb on PATH rather than /usr/local/bin/pkb
      const logsPath: string[] = [];
      const resolvedFromPath = resolvePkbExecutable('pkb', (m) => logsPath.push(m));
      assert.strictEqual(
        resolvedFromPath,
        newerPathPkb,
        `Expected pkb from first PATH entry (${newerPathPkb}), not /usr/local/bin/pkb; got: ${resolvedFromPath}`
      );

      // Case 2: Explicit user setting for mem.lsp.executable wins over PATH
      const logsExplicit: string[] = [];
      const resolvedExplicit = resolvePkbExecutable(explicitPkb, (m) => logsExplicit.push(m));
      assert.strictEqual(
        resolvedExplicit,
        explicitPkb,
        `Explicit user setting (${explicitPkb}) must win over PATH (${newerPathPkb}); got: ${resolvedExplicit}`
      );
    } finally {
      process.env.PATH = origPath;
      process.env.HOME = origHome;
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

  it('computes inline hints with target title, status, and weight', () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'pkb-inlay-test-'));
    try {
      const taskPath = path.join(tmpPkb, 'task-inlay.md');
      fs.writeFileSync(
        taskPath,
        `---
id: task-inlay
title: Inlay Hint Target
type: task
status: ready
weight: 2.5
---
Body text.
`
      );

      const index = new PkbPreviewIndex(tmpPkb);
      const docText = 'Check [[task-inlay]] today.';
      const hints = computeInlayHints(docText, index);

      assert.strictEqual(hints.length, 1);
      assert.strictEqual(hints[0].position.line, 0);
      assert.strictEqual(hints[0].position.character, 20);
      assert.ok(hints[0].label.includes('Inlay Hint Target'), 'Hint should have target title');
      assert.ok(hints[0].label.includes('ready'), 'Hint should have status');
      assert.ok(hints[0].label.includes('2.5') || hints[0].label.includes('w:'), 'Hint should have weight');
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });

  it('computes semantic tokens with distinct types for task states and node types', () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'pkb-sem-test-'));
    try {
      fs.writeFileSync(
        path.join(tmpPkb, 't-ready.md'),
        '---\nid: t-ready\ntitle: Ready\ntype: task\nstatus: ready\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 't-done.md'),
        '---\nid: t-done\ntitle: Done\ntype: task\nstatus: done\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 't-blocked.md'),
        '---\nid: t-blocked\ntitle: Blocked\ntype: task\nstatus: blocked\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'n-info.md'),
        '---\nid: n-info\ntitle: Info\ntype: info\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'n-index.md'),
        '---\nid: n-index\ntitle: Index\ntype: index\n---\n'
      );

      const index = new PkbPreviewIndex(tmpPkb);
      const docText = '[[t-ready]] [[t-done]] [[t-blocked]] [[n-info]] [[n-index]]';
      const tokens = computeSemanticTokens(docText, index);

      assert.strictEqual(tokens.length, 5);
      // ready: 0, done: 1, blocked: 2, info: 4, index: 5
      assert.strictEqual(tokens[0].tokenType, 0, 'Ready task has tokenType 0');
      assert.strictEqual(tokens[1].tokenType, 1, 'Done task has tokenType 1');
      assert.strictEqual(tokens[2].tokenType, 2, 'Blocked task has tokenType 2');
      assert.strictEqual(tokens[3].tokenType, 4, 'Info node has tokenType 4');
      assert.strictEqual(tokens[4].tokenType, 5, 'Index node has tokenType 5');
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });

  it('computes header bar CodeLens with parent by title, backlinks, subtasks by status, and dispatch action', () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'pkb-lens-test-'));
    try {
      fs.writeFileSync(
        path.join(tmpPkb, 'parent-node.md'),
        '---\nid: parent-node\ntitle: Epic Root\ntype: task\nstatus: in_progress\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'sub-ready.md'),
        '---\nid: sub-ready\ntitle: Child 1\ntype: task\nstatus: ready\nparent: main-doc\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'sub-done.md'),
        '---\nid: sub-done\ntitle: Child 2\ntype: task\nstatus: done\nparent: main-doc\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'linker.md'),
        '---\nid: linker\ntitle: Linker\ntype: note\n---\nReferences [[main-doc]] here.\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'main-doc.md'),
        '---\nid: main-doc\ntitle: Main Document\ntype: task\nstatus: in_progress\nparent: parent-node\n---\nMain content\n'
      );

      const index = new PkbPreviewIndex(tmpPkb);
      const docText = fs.readFileSync(path.join(tmpPkb, 'main-doc.md'), 'utf8');
      const lenses = computeHeaderCodeLenses(path.join(tmpPkb, 'main-doc.md'), docText, index);

      assert.strictEqual(lenses.length, 4, 'Header bar should have 4 CodeLens items');

      const titles = lenses.map((l) => l.command?.title || '');
      assert.ok(
        titles.some((t) => t.includes('Parent: Epic Root')),
        `Should show parent by title, got: ${titles.join(' | ')}`
      );
      assert.ok(
        titles.some((t) => t.includes('backlink')),
        `Should show backlink count, got: ${titles.join(' | ')}`
      );
      assert.ok(
        titles.some((t) => t.includes('Subtasks:') && t.includes('ready') && t.includes('done')),
        `Should show subtasks by status, got: ${titles.join(' | ')}`
      );
      assert.ok(
        lenses.some((l) => l.command?.command === 'mem.dispatch'),
        `Should include dispatch command action, got: ${titles.join(' | ')}`
      );
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });

  it('computes gutter badges on lines containing task links', () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'pkb-gutter-test-'));
    try {
      fs.writeFileSync(
        path.join(tmpPkb, 'task-r.md'),
        '---\nid: task-r\ntitle: Task Ready\ntype: task\nstatus: ready\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'task-b.md'),
        '---\nid: task-b\ntitle: Task Blocked\ntype: task\nstatus: blocked\n---\n'
      );
      fs.writeFileSync(
        path.join(tmpPkb, 'note-plain.md'),
        '---\nid: note-plain\ntitle: Plain Note\ntype: note\n---\n'
      );

      const index = new PkbPreviewIndex(tmpPkb);
      const text =
        'Line 0: [[task-r]] here\n' +
        'Line 1: [[note-plain]] only\n' +
        'Line 2: [[task-b]] blocker\n' +
        'Line 3: no links';

      const badges = computeGutterBadges(text, index);
      assert.strictEqual(badges.length, 2, 'Should only produce gutter badges for lines with task links');
      assert.strictEqual(badges[0].line, 0);
      assert.strictEqual(badges[0].badge, 'ready');
      assert.strictEqual(badges[1].line, 2);
      assert.strictEqual(badges[1].badge, 'blocked');
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });
});
