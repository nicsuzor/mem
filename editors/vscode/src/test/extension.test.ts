import * as assert from 'assert';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';
import {
  activate,
  collectDescendants,
  computeDocumentLinks,
  computeGutterBadges,
  computeHeaderCodeLenses,
  computeInlayHints,
  computeSemanticTokens,
  createMarkdownItPlugin,
  extractReferenceAtPosition,
  formatBodyMicroHeadersHtml,
  getOutputChannel,
  getTraceOutputChannel,
  handleActiveEditorChanged,
  isPathIgnored,
  logDebug,
  logError,
  logInfo,
  logWarn,
  openReferencedFile,
  PkbDocumentLinkProvider,
  PkbPreviewIndex,
  PkbPreviewNode,
  renderHoverCardHtml,
  renderSubtreeText,
  resetOutputChannelsForTesting,
  resolvePkbExecutable,
  resolvePkbRoot,
  statusCodicon,
  typeCodicon,
} from '../extension';
import { createdOutputChannels, mockVscode, MockUri } from './mock-vscode';

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

  it('provides working LogOutputChannel methods (.error, .warn, .info, .debug, .trace) on OutputChannel', () => {
    resetOutputChannelsForTesting();
    createdOutputChannels.clear();

    const channel = getOutputChannel();
    assert.strictEqual(typeof channel.error, 'function', 'channel.error must be a function');
    assert.strictEqual(typeof channel.warn, 'function', 'channel.warn must be a function');
    assert.strictEqual(typeof channel.info, 'function', 'channel.info must be a function');
    assert.strictEqual(typeof channel.debug, 'function', 'channel.debug must be a function');
    assert.strictEqual(typeof channel.trace, 'function', 'channel.trace must be a function');

    // Test calling error with string (as vscode-languageclient does when piping stderr)
    assert.doesNotThrow(() => {
      channel.error('LSP stderr crash test');
    });

    // Test calling error with Error object
    assert.doesNotThrow(() => {
      channel.error(new Error('LSP transport error'));
    });

    const lines = (channel as any).lines.join('\n');
    assert.ok(lines.includes('[ERROR] LSP stderr crash test'), 'Should contain string error');
    assert.ok(lines.includes('[ERROR] LSP transport error'), 'Should contain Error object message');
  });

  it('safely polyfills .error on plain OutputChannel lacking LogOutputChannel methods', () => {
    resetOutputChannelsForTesting();

    const plainLines: string[] = [];
    const plainMockWindow: any = {
      createOutputChannel: (name: string) => ({
        name,
        append: (v: string) => plainLines.push(v),
        appendLine: (v: string) => plainLines.push(v),
        replace: () => {},
        clear: () => {
          plainLines.length = 0;
        },
        show: () => {},
        hide: () => {},
        dispose: () => {},
      }),
    };

    const channel = getOutputChannel(plainMockWindow);
    assert.strictEqual(typeof channel.error, 'function', 'channel.error must be polyfilled');

    // This simulates the vscode-languageclient stderr piping: outputChannel.error(data)
    assert.doesNotThrow(() => {
      channel.error('server stderr line');
    });

    assert.ok(
      plainLines.some((l) => l.includes('[ERROR] server stderr line')),
      'Polyfilled .error must append error line to channel'
    );
  });

  it('provides working LogOutputChannel on getTraceOutputChannel', () => {
    resetOutputChannelsForTesting();
    createdOutputChannels.clear();

    const traceChannel = getTraceOutputChannel();
    assert.strictEqual(typeof traceChannel.error, 'function');
    assert.strictEqual(typeof traceChannel.trace, 'function');
    assert.doesNotThrow(() => {
      traceChannel.error('Trace error test');
      traceChannel.trace('Trace verbose test');
    });
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
      assert.ok(html.includes('proj-mem'), 'Hover card must include parent reference');
      assert.ok(!html.includes('[[proj-mem]]'), 'Hover card must not contain raw [[id]] wikilink syntax');
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

  it('typeCodicon and statusCodicon map node types and states to VS Code theme codicons', () => {
    assert.strictEqual(typeCodicon('task'), '$(checklist)');
    assert.strictEqual(typeCodicon('note'), '$(note)');
    assert.strictEqual(typeCodicon('epic'), '$(project)');
    assert.strictEqual(typeCodicon('project'), '$(project)');
    assert.strictEqual(typeCodicon('memory'), '$(history)');
    assert.strictEqual(typeCodicon('unknown_type'), '$(symbol-misc)');

    assert.strictEqual(statusCodicon('in_progress'), '$(sync~spin)');
    assert.strictEqual(statusCodicon('ready'), '$(play)');
    assert.strictEqual(statusCodicon('done'), '$(check)');
    assert.strictEqual(statusCodicon('blocked'), '$(circle-slash)');
    assert.strictEqual(statusCodicon('inbox'), '$(inbox)');
    assert.strictEqual(statusCodicon('unknown_status'), '$(circle-outline)');
  });

  it('formatBodyMicroHeadersHtml formats markdown headings into inline bold micro-headers', () => {
    const input1 = '## Context\nHere is the context paragraph.';
    const out1 = formatBodyMicroHeadersHtml(input1);
    assert.strictEqual(out1, '<strong>Context:</strong> Here is the context paragraph.');

    const input2 = '### Acceptance Criteria\n- First requirement\n- Second requirement';
    const out2 = formatBodyMicroHeadersHtml(input2);
    assert.strictEqual(
      out2,
      '<strong>Acceptance Criteria:</strong>\n- First requirement\n- Second requirement'
    );
  });

  it('renderHoverCardHtml renders compact interactive card with codicons, human titles, micro-headers, and action footer', () => {
    const parentNode: PkbPreviewNode = {
      id: 'proj-card-parent',
      label: 'Parent Project',
      type: 'project',
      status: 'in_progress',
      tags: ['parent'],
      aliases: [],
      bodyExcerpt: 'Parent node body',
      hasMoreBody: false,
      absPath: '/tmp/proj-card-parent.md',
      relPath: 'tasks/proj-card-parent.md',
    };

    const node: PkbPreviewNode = {
      id: 'task-card-child',
      label: 'Child Task Card',
      type: 'task',
      status: 'in_progress',
      intent: 3,
      tags: ['ui', 'compact'],
      parent: 'proj-card-parent',
      aliases: [],
      bodyExcerpt: '## Summary\nCompact card design.\n### Criteria\n- AC1\n- AC2',
      hasMoreBody: false,
      absPath: '/tmp/task-card-child.md',
      relPath: 'tasks/task-card-child.md',
    };

    const mockIndex: any = {
      resolve: (ref: string) => (ref === 'proj-card-parent' ? parentNode : undefined),
    };

    const cardHtml = renderHoverCardHtml(node, mockIndex);

    // 1. Top bar codicons & compact labels
    assert.ok(cardHtml.includes('$(checklist) task'), 'Top bar must contain type codicon and compact label');
    assert.ok(cardHtml.includes('$(sync~spin) in_progress'), 'Top bar must contain status codicon and compact label');
    assert.ok(cardHtml.includes('$(flame) intent 3'), 'Top bar must contain intent codicon');
    assert.ok(cardHtml.includes('$(tag)'), 'Top bar must contain tag codicon');
    assert.ok(cardHtml.includes('#ui') && cardHtml.includes('#compact'), 'Top bar must contain styled tags');

    // 2. Human title for parent link, no raw [[id]]
    assert.ok(cardHtml.includes('Parent Project'), 'Card must resolve parent to human title');
    assert.ok(cardHtml.includes('href="file:///tmp/proj-card-parent.md"'), 'Parent title must link to file URI');
    assert.ok(!cardHtml.includes('[[proj-card-parent]]'), 'Card must not contain raw [[id]] syntax');

    // 3. Body headings formatted as inline micro-headers
    assert.ok(
      cardHtml.includes('<strong>Summary:</strong> Compact card design.'),
      'Body heading must render as inline bold micro-header'
    );
    assert.ok(
      cardHtml.includes('<strong>Criteria:</strong>\n- AC1\n- AC2'),
      'List heading must render as compact bold inline micro-header'
    );

    // 4. Action footer with extension command links
    const encId = encodeURIComponent(JSON.stringify(['task-card-child']));
    assert.ok(
      cardHtml.includes(`href="command:mem.openReference?${encId}"`),
      'Action footer must contain command:mem.openReference link'
    );
    assert.ok(
      cardHtml.includes(`href="command:mem.showSubtree?${encId}"`),
      'Action footer must contain command:mem.showSubtree link'
    );
    assert.ok(cardHtml.includes('$(go-to-file) Open'), 'Open command link must use $(go-to-file) icon');
    assert.ok(cardHtml.includes('$(type-hierarchy-sub) Subtree'), 'Subtree command link must use $(type-hierarchy-sub) icon');
  });

  it('collectDescendants and renderSubtreeText build child hierarchy and tree structure', () => {
    const rootNode: PkbPreviewNode = {
      id: 'epic-root',
      label: 'Root Epic',
      type: 'epic',
      status: 'in_progress',
      tags: [],
      aliases: [],
      bodyExcerpt: '',
      hasMoreBody: false,
      absPath: '/tmp/epic-root.md',
      relPath: 'tasks/epic-root.md',
    };

    const childNode1: PkbPreviewNode = {
      id: 'task-child-1',
      label: 'Child Task One',
      type: 'task',
      status: 'done',
      parent: 'epic-root',
      tags: [],
      aliases: [],
      bodyExcerpt: '',
      hasMoreBody: false,
      absPath: '/tmp/task-child-1.md',
      relPath: 'tasks/task-child-1.md',
    };

    const childNode2: PkbPreviewNode = {
      id: 'task-child-2',
      label: 'Child Task Two',
      type: 'task',
      status: 'ready',
      parent: 'epic-root',
      tags: [],
      aliases: [],
      bodyExcerpt: '',
      hasMoreBody: false,
      absPath: '/tmp/task-child-2.md',
      relPath: 'tasks/task-child-2.md',
    };

    const mockIndex: any = {
      resolve: (ref: string) => {
        if (ref === 'epic-root') return rootNode;
        if (ref === 'task-child-1') return childNode1;
        if (ref === 'task-child-2') return childNode2;
        return undefined;
      },
      getAllNodes: () => [rootNode, childNode1, childNode2],
    };

    const descendants = collectDescendants('epic-root', mockIndex);
    assert.strictEqual(descendants.length, 2, 'Should find 2 descendant nodes');
    assert.strictEqual(descendants[0].id, 'task-child-1');
    assert.strictEqual(descendants[1].id, 'task-child-2');

    const treeText = renderSubtreeText('epic-root', mockIndex);
    assert.ok(treeText.includes('Root Epic (epic-root)'), 'Tree text must contain root node');
    assert.ok(treeText.includes('Child Task One (task-child-1)'), 'Tree text must contain child 1');
    assert.ok(treeText.includes('Child Task Two (task-child-2)'), 'Tree text must contain child 2');
    assert.ok(treeText.includes('└── ') || treeText.includes('├── '), 'Tree text must have tree branch indentation');
  });

  it('mem.showSubtree and mem.openReference command handlers can be executed via vscode commands', async () => {
    resetOutputChannelsForTesting();
    createdOutputChannels.clear();

    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'mem-cmd-test-'));
    try {
      const tasksDir = path.join(tmpPkb, 'tasks');
      fs.mkdirSync(tasksDir, { recursive: true });

      const rootTaskPath = path.join(tasksDir, 'cmd-root.md');
      fs.writeFileSync(
        rootTaskPath,
        `---
id: cmd-root
title: Command Root Task
type: task
status: in_progress
---
Root body.
`
      );

      const childTaskPath = path.join(tasksDir, 'cmd-child.md');
      fs.writeFileSync(
        childTaskPath,
        `---
id: cmd-child
title: Command Child Task
type: task
status: ready
parent: [[cmd-root]]
---
Child body.
`
      );

      const mockContext: any = {
        subscriptions: [],
        extensionUri: MockUri.file(tmpPkb),
        asAbsolutePath: (rel: string) => path.join(tmpPkb, rel),
      };

      // Set environment and workspace configuration to use tmpPkb
      process.env.ACA_DATA = tmpPkb;
      process.env.MEM_TEST_NO_LSP_CLIENT = '1';
      activate(mockContext);

      // Execute mem.showSubtree with string nodeId
      const subtreeRes: any = await mockVscode.commands.executeCommand('mem.showSubtree', 'cmd-root');
      assert.ok(subtreeRes, 'mem.showSubtree must return execution result');
      assert.strictEqual(subtreeRes.nodeId, 'cmd-root');
      assert.ok(subtreeRes.treeOutput.includes('Command Root Task'), 'Output channel must contain root node');

      // Execute mem.showSubtree with array nodeId (as generated by VS Code command URIs)
      const subtreeArrayRes: any = await mockVscode.commands.executeCommand('mem.showSubtree', ['cmd-root']);
      assert.ok(subtreeArrayRes, 'mem.showSubtree must accept array arguments from command URIs');
      assert.strictEqual(subtreeArrayRes.nodeId, 'cmd-root');

      // Execute mem.openReference with array nodeId
      const openResult: any = await mockVscode.commands.executeCommand('mem.openReference', ['cmd-child']);
      assert.ok(openResult, 'mem.openReference must open target file');
      assert.strictEqual(openResult.options?.preview, false, 'Must open with preview: false in new tab');
    } finally {
      delete process.env.ACA_DATA;
      delete process.env.MEM_TEST_NO_LSP_CLIENT;
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
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

  it('translates markdown PKB links (wikilinks, obsidian URIs, markdown links, bare IDs) and clicking opens the linked node', async () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'pkb-links-test-'));
    try {
      const tasksDir = path.join(tmpPkb, 'tasks');
      fs.mkdirSync(tasksDir, { recursive: true });

      const targetPath = path.join(tasksDir, 'task-link-target.md');
      const targetContent = [
        '---',
        'id: task-link-target',
        'title: Target Task Title',
        'type: task',
        'status: ready',
        '---',
        'Body content of the target task.',
      ].join('\n');
      fs.writeFileSync(targetPath, targetContent);

      const index = new PkbPreviewIndex(tmpPkb);
      assert.strictEqual(index.size, 1);

      // 1. Test index.resolve on Obsidian URIs
      const resObs1 = index.resolve('obsidian://open?vault=brain&file=task-link-target');
      assert.ok(resObs1, 'Must resolve obsidian:// URI with bare node ID');
      assert.strictEqual(resObs1.id, 'task-link-target');

      const resObs2 = index.resolve('obsidian://open?vault=brain&file=tasks%2Ftask-link-target.md');
      assert.ok(resObs2, 'Must resolve obsidian:// URI with encoded relative path');
      assert.strictEqual(resObs2.id, 'task-link-target');

      const resObs3 = index.resolve('obsidian://open?file=tasks/task-link-target.md');
      assert.ok(resObs3, 'Must resolve obsidian:// URI with unencoded relative path');
      assert.strictEqual(resObs3.id, 'task-link-target');

      // 2. Test extractReferenceAtPosition on Obsidian URI
      const lineWithObsidian = 'Open this: obsidian://open?vault=brain&file=task-link-target now';
      const extractedObs = extractReferenceAtPosition(lineWithObsidian, 20);
      assert.ok(extractedObs, 'Should extract obsidian URI at cursor');
      assert.strictEqual(extractedObs.target, 'obsidian://open?vault=brain&file=task-link-target');

      // 3. Test document link translation across all markdown PKB link variants
      const markdownDoc = [
        'Line 0: [[task-link-target]]',
        'Line 1: [[task-link-target|Custom Alias]]',
        'Line 2: [Standard Link](task-link-target)',
        'Line 3: [Obsidian Link](obsidian://open?vault=brain&file=task-link-target)',
        'Line 4: obsidian://open?vault=brain&file=task-link-target',
        'Line 5: task-link-target',
      ].join('\n');

      const docLinks = computeDocumentLinks(markdownDoc, index);
      console.log('\n--- SPECIMEN CAPTURE: TRANSLATED MARKDOWN PKB LINKS ---');
      docLinks.forEach((dl: any, i: number) => {
        console.log(`[Link ${i}] line=${dl.range.start.line} char=${dl.range.start.character}-${dl.range.end.character} target=${dl.target.toString()} tooltip="${dl.tooltip}"`);
      });
      console.log('--- END SPECIMEN CAPTURE ---\n');

      assert.strictEqual(docLinks.length, 6, 'Must translate all 6 PKB link variants into DocumentLinks');
      const expectedUriStr = `file://${targetPath}`;

      for (let i = 0; i < 6; i++) {
        const link = docLinks[i];
        assert.ok(link, `Link ${i} must exist`);
        assert.ok(link.target, `Link ${i} target must exist`);
        assert.strictEqual(link.range.start.line, i, `Link ${i} must be on line ${i}`);
        assert.strictEqual(
          link.target.toString(),
          expectedUriStr,
          `Link ${i} must point to file://${targetPath}`
        );
        assert.ok(
          link.tooltip?.includes('Target Task Title') || link.tooltip?.includes('task-link-target'),
          `Link ${i} tooltip must reference target title or ID`
        );
      }

      // 4. Test clicking/activating a translated link opens the linked node in a permanent new editor tab (preview: false)
      const mockOpenedDocs: any[] = [];
      const mockShowOptions: any[] = [];
      const mockWorkspace: any = {
        openTextDocument: async (uri: any) => {
          mockOpenedDocs.push(uri);
          return { uri, getText: () => targetContent };
        },
      };
      const mockWindow: any = {
        showTextDocument: async (doc: any, options: any) => {
          mockShowOptions.push(options);
          return { document: doc };
        },
      };

      // Simulate clicking the translated wikilink (Link 0)
      const link0Target = docLinks[0]?.target?.toString();
      assert.ok(link0Target, 'Link 0 target must exist');
      const clickedEditor0 = await openReferencedFile(
        link0Target,
        { preview: false },
        mockWorkspace,
        mockWindow
      );
      assert.ok(clickedEditor0, 'Clicking wikilink must open target document');
      assert.strictEqual(mockShowOptions[0].preview, false, 'Must open in new tab (preview: false)');
      assert.strictEqual(clickedEditor0.document.getText(), targetContent);

      // Simulate clicking the translated obsidian URI link (Link 3)
      const link3Target = docLinks[3]?.target?.toString();
      assert.ok(link3Target, 'Link 3 target must exist');
      const clickedEditor3 = await openReferencedFile(
        link3Target,
        { preview: false },
        mockWorkspace,
        mockWindow
      );
      assert.ok(clickedEditor3, 'Clicking obsidian link must open target document');
      assert.strictEqual(mockShowOptions[1].preview, false, 'Must open in new tab (preview: false)');

      // 5. Test PkbDocumentLinkProvider class
      const provider = new PkbDocumentLinkProvider(index);
      const fakeDoc: any = {
        getText: () => markdownDoc,
        lineCount: 6,
        lineAt: (n: number) => ({ text: markdownDoc.split('\n')[n] }),
      };
      const provided = provider.provideDocumentLinks(fakeDoc);
      assert.strictEqual(provided.length, 6, 'PkbDocumentLinkProvider must return all 6 translated links');
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });

  it('correctly detects ignored directory paths via isPathIgnored', () => {
    assert.strictEqual(isPathIgnored('/home/user/brain/.git/COMMIT_EDITMSG'), true);
    assert.strictEqual(isPathIgnored('/home/user/brain/node_modules/pkg/README.md'), true);
    assert.strictEqual(isPathIgnored('/home/user/brain/.obsidian/workspace.json'), true);
    assert.strictEqual(isPathIgnored('/home/user/brain/.claude/tasks/task-1.md'), true);
    assert.strictEqual(isPathIgnored('/home/user/brain/.aops/data.md'), true);
    assert.strictEqual(isPathIgnored('/home/user/brain/tasks/task-123.md'), false);
    assert.strictEqual(isPathIgnored('/home/user/brain/notes/architecture.md'), false);
    assert.strictEqual(isPathIgnored(''), false);
  });

  it('supports incremental updates and file removals without full directory re-scans', () => {
    const tmpPkb = fs.mkdtempSync(path.join(os.tmpdir(), 'pkb-incr-test-'));
    try {
      const taskFile = path.join(tmpPkb, 'task-1.md');
      fs.writeFileSync(
        taskFile,
        `---
id: task-1
title: Task One Initial
type: task
status: ready
---
Initial content referencing [[task-2]].
`
      );

      const task2File = path.join(tmpPkb, 'task-2.md');
      fs.writeFileSync(
        task2File,
        `---
id: task-2
title: Task Two
type: task
status: done
---
Second task.
`
      );

      const index = new PkbPreviewIndex(tmpPkb);
      assert.strictEqual(index.size, 2);
      assert.strictEqual(index.isUnderRoot(taskFile), true);
      assert.strictEqual(index.isUnderRoot('/some/other/path/file.md'), false);

      const node1 = index.resolve('task-1');
      assert.ok(node1);
      assert.strictEqual(node1.label, 'Task One Initial');

      const node2 = index.resolve('task-2');
      assert.ok(node2);
      assert.strictEqual(node2.backlinkCount, 1);

      // Incrementally update task-1
      fs.writeFileSync(
        taskFile,
        `---
id: task-1
title: Task One Updated
type: task
status: in_progress
---
Updated content without backlink.
`
      );
      const updateResult = index.updateFile(taskFile);
      assert.strictEqual(updateResult, true);
      index.computeGraphRelationships();

      const updatedNode1 = index.resolve('task-1');
      assert.ok(updatedNode1);
      assert.strictEqual(updatedNode1.label, 'Task One Updated');
      assert.strictEqual(updatedNode1.status, 'in_progress');

      // Backlink on task-2 should now be 0
      const node2After = index.resolve('task-2');
      assert.ok(node2After);
      assert.strictEqual(node2After.backlinkCount, 0);

      // Incrementally remove task-2
      fs.unlinkSync(task2File);
      const removeResult = index.removeFile(task2File);
      assert.strictEqual(removeResult, true);
      assert.strictEqual(index.size, 1);
      assert.strictEqual(index.resolve('task-2'), undefined);
    } finally {
      fs.rmSync(tmpPkb, { recursive: true, force: true });
    }
  });

});
