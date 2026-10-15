import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';
import * as vscode from 'vscode';
import {
  CloseAction,
  ErrorAction,
  ErrorHandlerResult,
  CloseHandlerResult,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  State,
  TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;
let traceOutputChannel: vscode.OutputChannel | undefined;
let statusBarItem: vscode.StatusBarItem | undefined;
let previewIndex: PkbPreviewIndex | undefined;
let currentPkbRoot: string = '.';
let currentExecutable: string = 'pkb';
let currentExtVersion: string = '0.1.3';
let serverStatus: 'stopped' | 'starting' | 'running' | 'error' = 'stopped';
let lastServerError: string | undefined;

const IGNORED_DIRS = new Set([
  '.git',
  'node_modules',
  '.obsidian',
  '.venv',
  'target',
  '.claude',
  '.aops',
]);

/**
 * Get or create the primary "Mem PKB LSP" OutputChannel.
 */
export function getOutputChannel(windowObj: typeof vscode.window = vscode.window): vscode.OutputChannel {
  if (!outputChannel && typeof windowObj?.createOutputChannel === 'function') {
    outputChannel = windowObj.createOutputChannel('Mem PKB LSP');
  }
  if (!outputChannel) {
    // Fallback no-op channel if window.createOutputChannel is unavailable
    const lines: string[] = [];
    outputChannel = {
      name: 'Mem PKB LSP',
      append: (val: string) => {
        lines.push(val);
      },
      appendLine: (val: string) => {
        lines.push(val);
      },
      replace: (val: string) => {
        lines.length = 0;
        lines.push(val);
      },
      clear: () => {
        lines.length = 0;
      },
      show: () => {},
      hide: () => {},
      dispose: () => {},
    } as unknown as vscode.OutputChannel;
  }
  return outputChannel;
}

/**
 * Get or create the "Mem PKB LSP Trace" OutputChannel.
 */
export function getTraceOutputChannel(
  windowObj: typeof vscode.window = vscode.window
): vscode.OutputChannel {
  if (!traceOutputChannel && typeof windowObj?.createOutputChannel === 'function') {
    traceOutputChannel = windowObj.createOutputChannel('Mem PKB LSP Trace');
  }
  if (!traceOutputChannel) {
    traceOutputChannel = getOutputChannel(windowObj);
  }
  return traceOutputChannel;
}

/**
 * Reset cached OutputChannels (used in unit tests).
 */
export function resetOutputChannelsForTesting(): void {
  outputChannel = undefined;
  traceOutputChannel = undefined;
  lastServerError = undefined;
  serverStatus = 'stopped';
}

function formatLogLine(level: 'INFO' | 'WARN' | 'ERROR' | 'DEBUG', message: string): string {
  return `[${new Date().toISOString()}] [${level}] ${message}`;
}

export function logInfo(message: string): string {
  const line = formatLogLine('INFO', message);
  getOutputChannel().appendLine(line);
  return line;
}

export function logWarn(message: string): string {
  const line = formatLogLine('WARN', message);
  getOutputChannel().appendLine(line);
  return line;
}

export function logError(message: string, err?: unknown): string {
  let details = message;
  if (err !== undefined && err !== null) {
    if (err instanceof Error) {
      details += `: ${err.message}`;
      if (err.stack) {
        details += `\n${err.stack}`;
      }
    } else {
      details += `: ${String(err)}`;
    }
  }
  const line = formatLogLine('ERROR', details);
  getOutputChannel().appendLine(line);
  return line;
}

export function logDebug(message: string): string {
  const line = formatLogLine('DEBUG', message);
  const debugEnabled = vscode.workspace
    ?.getConfiguration?.('mem.lsp')
    ?.get<boolean>('debug', true);
  if (debugEnabled !== false) {
    getOutputChannel().appendLine(line);
  }
  return line;
}

/**
 * Expand `~`, `$VAR`, and `${VAR}` in path strings.
 */
export function expandPathVariables(
  rawPath: string,
  env: NodeJS.ProcessEnv = process.env
): string {
  const home = env.HOME || env.USERPROFILE || os.homedir() || '';
  let expanded = rawPath.trim();
  if (expanded === '~') {
    expanded = home;
  } else if (expanded.startsWith('~/') || expanded.startsWith('~\\')) {
    expanded = path.join(home, expanded.slice(2));
  }
  expanded = expanded.replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)/g, (_m, g1, g2) => {
    const varName = g1 || g2;
    return env[varName] ?? '';
  });
  return expanded;
}

/**
 * Resolve the `pkb` binary path, logging PATH checks.
 */
export function resolvePkbExecutable(
  configured: string,
  log: (msg: string) => void = logInfo
): string {
  if (configured && configured.trim() !== '' && configured.trim() !== 'pkb') {
    const expanded = expandPathVariables(configured);
    log(`Using configured mem.lsp.executable: ${expanded}`);
    if (!fs.existsSync(expanded)) {
      logWarn(`Configured mem.lsp.executable does not exist on disk: ${expanded}`);
    }
    return expanded;
  }

  const pathEnv = process.env.PATH || '';
  const pathDirs = pathEnv
    .split(path.delimiter)
    .map((d) => d.trim())
    .filter(Boolean);
  const binNames = process.platform === 'win32' ? ['pkb.exe', 'pkb'] : ['pkb'];

  for (const dir of pathDirs) {
    for (const binName of binNames) {
      const candidate = path.join(dir, binName);
      log(`Checking pkb executable candidate on PATH: ${candidate}`);
      if (fs.existsSync(candidate)) {
        log(`Resolved pkb executable on PATH: ${candidate}`);
        return candidate;
      }
    }
  }

  const warnMsg = `Could not find pkb binary on PATH (${pathEnv}); falling back to bare 'pkb'`;
  logWarn(warnMsg);
  if (log !== logInfo) {
    log(`[WARN] ${warnMsg}`);
  }
  return 'pkb';
}

/**
 * Check whether a directory contains standard PKB markers.
 */
export function isLikelyPkbDirectory(dirPath: string): boolean {
  if (!dirPath || !fs.existsSync(dirPath)) {
    return false;
  }
  const markers = ['pkb_vectors.bin', 'tasks', 'notes', 'memories'];
  for (const marker of markers) {
    if (fs.existsSync(path.join(dirPath, marker))) {
      return true;
    }
  }
  return false;
}

/**
 * Resolve the PKB root directory with full diagnostic logging:
 * 1. Configured `mem.lsp.pkbRoot` (expanding `~` and `$VAR` / `${VAR}`)
 * 2. Any workspaceFolder that looks like a PKB root (`pkb_vectors.bin`, `tasks`, `notes`, `memories`)
 * 3. `env.ACA_DATA` (expanding `~` and `$VAR` / `${VAR}`)
 * 4. Default candidate directories (`~/brain`, `~/aca-data`) if they exist on disk
 * 5. First workspaceFolder or `.`
 */
export function resolvePkbRoot(
  configured?: string,
  workspaceFolders?: readonly { uri: { fsPath: string } }[],
  env: NodeJS.ProcessEnv = process.env,
  log: (msg: string) => void = logInfo
): string {
  const emitLog = (msg: string) => {
    log(msg);
    if (log !== logInfo) {
      logInfo(msg);
    }
  };

  const validateResolvedRoot = (resolved: string, source: string): string => {
    emitLog(`Resolved pkbRoot (${source}): ${resolved}`);
    if (!fs.existsSync(resolved)) {
      const msg = `Resolved pkbRoot (${source}) does not exist on disk: ${resolved}`;
      logWarn(msg);
      logError(msg);
      if (log !== logInfo) {
        log(`[WARN] ${msg}`);
      }
    }
    return resolved;
  };

  // 1. Configured mem.lsp.pkbRoot
  if (configured && configured.trim().length > 0) {
    const expanded = expandPathVariables(configured, env);
    return validateResolvedRoot(expanded, 'mem.lsp.pkbRoot');
  }

  // 2. Any workspace folder that contains PKB markers
  if (workspaceFolders && workspaceFolders.length > 0) {
    for (const folder of workspaceFolders) {
      const folderPath = folder?.uri?.fsPath;
      if (folderPath && isLikelyPkbDirectory(folderPath)) {
        return validateResolvedRoot(folderPath, 'workspaceFolder PKB marker');
      }
    }
  }

  // 3. ACA_DATA environment variable
  if (env.ACA_DATA && env.ACA_DATA.trim().length > 0) {
    const expanded = expandPathVariables(env.ACA_DATA, env);
    return validateResolvedRoot(expanded, 'ACA_DATA');
  }

  // 4. Default candidate directories (~/brain, ~/aca-data)
  const home = env.HOME || env.USERPROFILE || os.homedir() || '';
  if (home) {
    const candidates = [path.join(home, 'brain'), path.join(home, 'aca-data')];
    for (const cand of candidates) {
      emitLog(`Checking default pkbRoot candidate: ${cand}`);
      if (fs.existsSync(cand)) {
        return validateResolvedRoot(cand, 'default candidate');
      }
    }
  }

  // 5. Fallback to first workspaceFolder or '.'
  const fallback = workspaceFolders?.[0]?.uri?.fsPath || '.';
  return validateResolvedRoot(fallback, 'fallback');
}

export interface PkbPreviewNode {
  id: string;
  label: string;
  type?: string;
  status?: string;
  intent?: string | number;
  tags: string[];
  parent?: string;
  aliases: string[];
  bodyExcerpt: string;
  hasMoreBody: boolean;
  absPath: string;
  relPath: string;
}

/**
 * Parse YAML frontmatter and body excerpt from a Markdown file's text.
 */
export function parseMarkdownNode(
  content: string,
  absPath: string,
  pkbRoot: string
): PkbPreviewNode {
  const normalized = content.replace(/\r\n/g, '\n');
  const stem = path.basename(absPath, path.extname(absPath));
  const relPath = path
    .relative(pkbRoot, absPath)
    .split(path.sep)
    .join('/');

  let fmText = '';
  let body = normalized;

  if (normalized.startsWith('---\n')) {
    const endIdx = normalized.indexOf('\n---', 4);
    if (endIdx !== -1) {
      fmText = normalized.slice(4, endIdx);
      const afterFence = normalized.indexOf('\n', endIdx + 4);
      body = afterFence !== -1 ? normalized.slice(afterFence + 1) : '';
    }
  }

  const fields: Record<string, any> = {};
  if (fmText) {
    const lines = fmText.split('\n');
    let currentListKey: string | null = null;
    for (const rawLine of lines) {
      const listMatch = /^\s*-\s+(.+)$/.exec(rawLine);
      if (listMatch && currentListKey) {
        if (!Array.isArray(fields[currentListKey])) {
          fields[currentListKey] = [];
        }
        fields[currentListKey].push(stripYamlQuotes(listMatch[1].trim()));
        continue;
      }

      const kvMatch = /^([A-Za-z0-9_-]+)\s*:\s*(.*)$/.exec(rawLine);
      if (kvMatch) {
        const key = kvMatch[1].trim();
        const valRaw = kvMatch[2].trim();
        if (valRaw === '') {
          currentListKey = key;
          fields[key] = [];
        } else if (valRaw.startsWith('[') && valRaw.endsWith(']')) {
          currentListKey = null;
          const inner = valRaw.slice(1, -1).trim();
          fields[key] = inner
            ? inner
                .split(',')
                .map((s) => stripYamlQuotes(s.trim()))
                .filter(Boolean)
            : [];
        } else {
          currentListKey = null;
          fields[key] = stripYamlQuotes(valRaw);
        }
      }
    }
  }

  const id = String(fields.id || stem).trim();
  const label = String(fields.title || fields.label || stem).trim();
  const type = fields.type ? String(fields.type).trim() : undefined;
  const status = fields.status ? String(fields.status).trim() : undefined;
  const intent =
    fields.intent !== undefined && fields.intent !== ''
      ? fields.intent
      : undefined;

  let tags: string[] = [];
  if (Array.isArray(fields.tags)) {
    tags = fields.tags.map((t: any) => String(t).trim().replace(/^#/, '')).filter(Boolean);
  } else if (typeof fields.tags === 'string' && fields.tags.trim()) {
    tags = fields.tags
      .split(/[,\s]+/)
      .map((t: string) => t.trim().replace(/^#/, ''))
      .filter(Boolean);
  }

  let aliases: string[] = [];
  for (const aliasKey of ['aliases', 'alias']) {
    const rawAlias = fields[aliasKey];
    if (Array.isArray(rawAlias)) {
      aliases.push(...rawAlias.map((a: any) => String(a).trim()).filter(Boolean));
    } else if (typeof rawAlias === 'string' && rawAlias.trim()) {
      aliases.push(rawAlias.trim());
    }
  }

  let parent: string | undefined;
  if (fields.parent) {
    parent = String(fields.parent)
      .trim()
      .replace(/^\[\[/, '')
      .replace(/\]\]$/, '')
      .trim();
  }

  const bodyTrimmed = body.trim();
  const bodyLines = bodyTrimmed ? bodyTrimmed.split('\n') : [];
  const bodyExcerpt = bodyLines.slice(0, 12).join('\n');
  const hasMoreBody = bodyLines.length > 12;

  return {
    id,
    label,
    type,
    status,
    intent,
    tags,
    parent,
    aliases,
    bodyExcerpt,
    hasMoreBody,
    absPath,
    relPath,
  };
}

function stripYamlQuotes(val: string): string {
  if (
    (val.startsWith('"') && val.endsWith('"')) ||
    (val.startsWith("'") && val.endsWith("'"))
  ) {
    return val.slice(1, -1);
  }
  return val;
}

/**
 * Synchronous in-memory index of PKB Markdown nodes for Markdown Preview rendering.
 */
export class PkbPreviewIndex {
  public pkbRoot: string;
  private nodes: PkbPreviewNode[] = [];
  private byId = new Map<string, PkbPreviewNode>();
  private byLowerKey = new Map<string, PkbPreviewNode>();
  private byAbsPath = new Map<string, PkbPreviewNode>();
  private bareIds = new Set<string>();

  constructor(pkbRoot?: string) {
    this.pkbRoot = pkbRoot || '.';
    if (pkbRoot) {
      this.loadFromDirectory(pkbRoot);
    }
  }

  public get size(): number {
    return this.nodes.length;
  }

  public getNodes(): readonly PkbPreviewNode[] {
    return this.nodes;
  }

  public loadFromDirectory(pkbRoot: string): number {
    this.pkbRoot = pkbRoot;
    this.nodes = [];
    this.byId.clear();
    this.byLowerKey.clear();
    this.byAbsPath.clear();
    this.bareIds.clear();

    if (!pkbRoot || !fs.existsSync(pkbRoot)) {
      logWarn(`PKB preview index skipped: pkbRoot does not exist on disk: ${pkbRoot}`);
      return 0;
    }

    const scanDir = (dir: string) => {
      let entries: fs.Dirent[];
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch (err) {
        logWarn(`Failed reading directory during PKB scan (${dir}): ${err instanceof Error ? err.message : String(err)}`);
        return;
      }

      for (const entry of entries) {
        if (entry.isDirectory()) {
          if (IGNORED_DIRS.has(entry.name)) {
            continue;
          }
          scanDir(path.join(dir, entry.name));
        } else if (entry.isFile() && entry.name.endsWith('.md')) {
          const fullPath = path.join(dir, entry.name);
          try {
            const text = fs.readFileSync(fullPath, 'utf8');
            const node = parseMarkdownNode(text, fullPath, pkbRoot);
            this.indexNode(node);
          } catch (err) {
            logWarn(`Failed indexing markdown file ${fullPath}: ${err instanceof Error ? err.message : String(err)}`);
          }
        }
      }
    };

    scanDir(pkbRoot);

    if (this.nodes.length === 0) {
      logWarn(`PKB preview index found 0 markdown nodes in pkbRoot: ${pkbRoot}`);
    } else {
      logInfo(`PKB preview index loaded ${this.nodes.length} nodes from ${pkbRoot}`);
    }

    return this.nodes.length;
  }

  public indexNode(node: PkbPreviewNode): void {
    this.nodes.push(node);
    this.byAbsPath.set(path.resolve(node.absPath), node);

    const stem = path.basename(node.absPath, '.md');
    const relNoExt = node.relPath.replace(/\.md$/i, '');

    this.byId.set(node.id, node);
    this.byLowerKey.set(node.id.toLowerCase(), node);
    this.bareIds.add(node.id);

    for (const alias of node.aliases) {
      if (!this.byId.has(alias)) {
        this.byId.set(alias, node);
      }
      if (!this.byLowerKey.has(alias.toLowerCase())) {
        this.byLowerKey.set(alias.toLowerCase(), node);
      }
      this.bareIds.add(alias);
    }

    if (!this.byId.has(stem)) {
      this.byId.set(stem, node);
    }
    if (!this.byLowerKey.has(stem.toLowerCase())) {
      this.byLowerKey.set(stem.toLowerCase(), node);
    }
    if (stem.includes('-') || stem.includes('_')) {
      this.bareIds.add(stem);
    }

    this.byId.set(node.relPath, node);
    this.byId.set(relNoExt, node);
    this.byLowerKey.set(node.relPath.toLowerCase(), node);
    this.byLowerKey.set(relNoExt.toLowerCase(), node);
  }

  public hasPath(filePath: string): boolean {
    const resolved = path.resolve(filePath);
    if (this.byAbsPath.has(resolved)) {
      return true;
    }
    const rootResolved = path.resolve(this.pkbRoot);
    return resolved.startsWith(rootResolved + path.sep);
  }

  public resolve(rawTarget: string): PkbPreviewNode | undefined {
    if (!rawTarget) {
      return undefined;
    }
    let clean = rawTarget.trim();
    if (clean.startsWith('[[') && clean.endsWith(']]')) {
      clean = clean.slice(2, -2).split('|')[0].trim();
    }
    const hashIdx = clean.indexOf('#');
    if (hashIdx !== -1) {
      clean = clean.slice(0, hashIdx).trim();
    }
    clean = clean.replace(/^\.\//, '');
    if (!clean) {
      return undefined;
    }

    const noExt = clean.replace(/\.md$/i, '');

    const exact =
      this.byId.get(clean) ||
      this.byId.get(noExt) ||
      this.byLowerKey.get(clean.toLowerCase()) ||
      this.byLowerKey.get(noExt.toLowerCase());
    if (exact) {
      return exact;
    }

    // Check suffix match on relative paths (e.g. "tasks/task-123.md" or "task-123.md")
    const lowerClean = clean.toLowerCase();
    const lowerNoExt = noExt.toLowerCase();
    for (const node of this.nodes) {
      const relLower = node.relPath.toLowerCase();
      const relNoExtLower = relLower.replace(/\.md$/i, '');
      if (
        relLower === lowerClean ||
        relNoExtLower === lowerNoExt ||
        relLower.endsWith('/' + lowerClean) ||
        relNoExtLower.endsWith('/' + lowerNoExt)
      ) {
        return node;
      }
    }

    return undefined;
  }

  public resolveBareId(token: string): PkbPreviewNode | undefined {
    if (!token || token.length < 3) {
      return undefined;
    }
    if (this.bareIds.has(token)) {
      return this.byId.get(token) || this.byLowerKey.get(token.toLowerCase());
    }
    // Also match tokens with PKB ID structure (contains '_' or '-' and resolves to a node ID/stem)
    if (token.includes('_') || token.includes('-')) {
      const node = this.byId.get(token) || this.byLowerKey.get(token.toLowerCase());
      if (node) {
        return node;
      }
    }
    return undefined;
  }
}

export function reloadIndex(pkbRoot?: string): PkbPreviewIndex {
  const rootToUse = pkbRoot || currentPkbRoot || '.';
  currentPkbRoot = rootToUse;
  if (!previewIndex) {
    previewIndex = new PkbPreviewIndex(rootToUse);
  } else {
    previewIndex.loadFromDirectory(rootToUse);
  }
  return previewIndex;
}

export function getPreviewIndex(): PkbPreviewIndex {
  if (!previewIndex) {
    previewIndex = new PkbPreviewIndex(currentPkbRoot);
  }
  return previewIndex;
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function formatPlainTooltip(node: PkbPreviewNode): string {
  const lines: string[] = [`${node.label} (${node.id})`];
  const meta: string[] = [];
  if (node.type) meta.push(`Type: ${node.type}`);
  if (node.status) meta.push(`Status: ${node.status}`);
  if (node.intent !== undefined) meta.push(`Intent: ${node.intent}`);
  if (meta.length > 0) lines.push(meta.join(' | '));
  if (node.tags.length > 0) {
    lines.push(`Tags: ${node.tags.map((t) => `#${t}`).join(' ')}`);
  }
  if (node.parent) {
    lines.push(`Parent: [[${node.parent}]]`);
  }
  if (node.bodyExcerpt) {
    lines.push('');
    lines.push(node.bodyExcerpt);
  }
  lines.push('');
  lines.push('Click to open in new editor tab');
  return lines.join('\n');
}

function renderHoverCardHtml(node: PkbPreviewNode): string {
  const parts: string[] = [];
  parts.push(
    `<span class="mem-pkb-hover-header">` +
      `<span class="mem-pkb-hover-title">${escapeHtml(node.label)}</span>` +
      `<code class="mem-pkb-hover-id">${escapeHtml(node.id)}</code>` +
      `</span>`
  );

  const badges: string[] = [];
  if (node.type) {
    badges.push(
      `<span class="mem-pkb-hover-badge"><strong>Type:</strong> ${escapeHtml(node.type)}</span>`
    );
  }
  if (node.status) {
    badges.push(
      `<span class="mem-pkb-hover-badge"><strong>Status:</strong> ${escapeHtml(node.status)}</span>`
    );
  }
  if (node.intent !== undefined) {
    badges.push(
      `<span class="mem-pkb-hover-badge"><strong>Intent:</strong> ${escapeHtml(String(node.intent))}</span>`
    );
  }
  if (badges.length > 0) {
    parts.push(`<span class="mem-pkb-hover-meta">${badges.join(' ')}</span>`);
  }

  if (node.tags.length > 0) {
    const tagSpans = node.tags
      .map((t) => `<span class="mem-pkb-hover-tag">#${escapeHtml(t)}</span>`)
      .join(' ');
    parts.push(`<span class="mem-pkb-hover-tags"><strong>Tags:</strong> ${tagSpans}</span>`);
  }

  if (node.parent) {
    parts.push(
      `<span class="mem-pkb-hover-parent"><strong>Parent:</strong> <code>[[${escapeHtml(node.parent)}]]</code></span>`
    );
  }

  if (node.bodyExcerpt) {
    const suffix = node.hasMoreBody ? '\n...' : '';
    parts.push(
      `<span class="mem-pkb-hover-body">${escapeHtml(node.bodyExcerpt + suffix)}</span>`
    );
  }

  parts.push(
    `<span class="mem-pkb-hover-footer">Click to open in new editor tab (${escapeHtml(node.relPath)})</span>`
  );

  return `<span class="mem-pkb-hover-card">${parts.join('')}</span>`;
}

function renderResolvedRefAnchor(node: PkbPreviewNode, displayHtml: string): string {
  const fileUri = `file://${node.absPath}`;
  const tooltip = formatPlainTooltip(node);
  const hoverCard = renderHoverCardHtml(node);
  return (
    `<a class="mem-pkb-ref" href="${escapeHtml(fileUri)}" ` +
    `data-pkb-id="${escapeHtml(node.id)}" ` +
    `data-pkb-path="${escapeHtml(node.absPath)}" ` +
    `data-pkb-uri="${escapeHtml(fileUri)}" ` +
    `title="${escapeHtml(tooltip)}">` +
    `<span class="mem-pkb-ref-label">${displayHtml}</span>` +
    `${hoverCard}` +
    `</a>`
  );
}

function renderUnresolvedRefAnchor(
  target: string,
  displayText: string,
  pkbRoot: string
): string {
  const diag = `Unresolved PKB reference: ${target} (pkbRoot: ${pkbRoot})`;
  const card =
    `<span class="mem-pkb-hover-card">` +
    `<span class="mem-pkb-hover-header">` +
    `<span class="mem-pkb-hover-title">Unresolved PKB Reference</span>` +
    `<code class="mem-pkb-hover-id">${escapeHtml(target)}</code>` +
    `</span>` +
    `<span class="mem-pkb-hover-body">Target node <code>${escapeHtml(target)}</code> was not found in PKB root: <code>${escapeHtml(pkbRoot)}</code></span>` +
    `</span>`;
  return (
    `<a class="mem-pkb-ref mem-pkb-unresolved" href="#" ` +
    `data-pkb-id="${escapeHtml(target)}" ` +
    `data-pkb-unresolved="true" ` +
    `title="${escapeHtml(diag)}">` +
    `<span class="mem-pkb-ref-label">${escapeHtml(displayText)}</span>` +
    `${card}` +
    `</a>`
  );
}

/**
 * Transform rendered HTML (from markdown-it or basic markdown renderer) to enrich:
 * 1. Standard markdown links `<a href="target">label</a>` whose target resolves in PkbPreviewIndex
 * 2. Inline `<code>[[...]]</code>` and `<code>bare_pkb_id</code>`
 * 3. Wikilinks `[[target]]` and `[[target|alias]]` in text
 * 4. Bare PKB IDs in text
 */
export function transformPreviewHtml(html: string, index: PkbPreviewIndex): string {
  const placeholders: string[] = [];
  const savePlaceholder = (replacement: string): string => {
    const token = `\u0000MEMPKB_${placeholders.length}_\u0000`;
    placeholders.push(replacement);
    return token;
  };

  // Protect <pre>...</pre>, <script>...</script>, <style>...</style> blocks
  let working = html.replace(
    /<(pre|script|style)\b[^>]*>[\s\S]*?<\/\1>/gi,
    (match) => savePlaceholder(match)
  );

  // 1. Transform existing <a ...>...</a> links (e.g. from [label](target))
  working = working.replace(
    /<a\b([^>]*)>([\s\S]*?)<\/a>/gi,
    (fullMatch, attrs: string, innerHtml: string) => {
      if (attrs.includes('mem-pkb-ref')) {
        return savePlaceholder(fullMatch);
      }
      const hrefMatch = /\bhref=(?:"([^"]*)"|'([^']*)')/i.exec(attrs);
      if (!hrefMatch) {
        return savePlaceholder(fullMatch);
      }
      const rawHref = (hrefMatch[1] ?? hrefMatch[2] ?? '').trim();
      if (
        !rawHref ||
        rawHref.startsWith('http://') ||
        rawHref.startsWith('https://') ||
        rawHref.startsWith('mailto:') ||
        rawHref.startsWith('#')
      ) {
        return savePlaceholder(fullMatch);
      }

      let decodedHref = rawHref;
      try {
        decodedHref = decodeURIComponent(rawHref);
      } catch {
        // ignore malformed percent encoding
      }

      const node = index.resolve(decodedHref);
      if (node) {
        return savePlaceholder(renderResolvedRefAnchor(node, innerHtml));
      }
      return savePlaceholder(fullMatch);
    }
  );

  // 2. Transform inline <code>...</code> containing a wikilink or bare PKB ID
  working = working.replace(/<code\b([^>]*)>([\s\S]*?)<\/code>/gi, (fullMatch, _attrs, inner) => {
    const trimmed = inner.trim();
    const wikiMatch = /^\[\[([^\]|]+)(?:\|([^\]]+))?\]\]$/.exec(trimmed);
    if (wikiMatch) {
      const target = wikiMatch[1].trim();
      const alias = wikiMatch[2]?.trim();
      const node = index.resolve(target);
      if (node) {
        return savePlaceholder(
          renderResolvedRefAnchor(node, `<code>${escapeHtml(alias || target)}</code>`)
        );
      }
      return savePlaceholder(
        renderUnresolvedRefAnchor(target, alias || target, index.pkbRoot)
      );
    }

    const node = index.resolveBareId(trimmed);
    if (node) {
      return savePlaceholder(
        renderResolvedRefAnchor(node, `<code>${escapeHtml(trimmed)}</code>`)
      );
    }

    return savePlaceholder(fullMatch);
  });

  // Protect any remaining HTML tags so we only transform text nodes
  working = working.replace(/<[^>]+>/g, (tag) => savePlaceholder(tag));

  // 3. Transform Wikilinks [[target]] and [[target|alias]] in plain text
  working = working.replace(/\[\[([^\]|\n]+)(?:\|([^\]\n]+))?\]\]/g, (_m, rawTarget, rawAlias) => {
    const target = String(rawTarget).trim();
    const alias = rawAlias !== undefined ? String(rawAlias).trim() : undefined;
    const display = alias || target;
    const node = index.resolve(target);
    if (node) {
      return savePlaceholder(renderResolvedRefAnchor(node, escapeHtml(display)));
    }
    return savePlaceholder(renderUnresolvedRefAnchor(target, display, index.pkbRoot));
  });

  // 4. Transform bare PKB IDs in plain text
  working = working.replace(/[a-zA-Z0-9_\-\.]+/g, (token) => {
    if (token.includes('\u0000MEMPKB_')) {
      return token;
    }
    let clean = token;
    let suffix = '';
    while (
      clean.endsWith('.') ||
      clean.endsWith(',') ||
      clean.endsWith(';') ||
      clean.endsWith(':')
    ) {
      suffix = clean.slice(-1) + suffix;
      clean = clean.slice(0, -1);
    }
    if (!clean) {
      return token;
    }
    const node = index.resolveBareId(clean);
    if (node) {
      return savePlaceholder(renderResolvedRefAnchor(node, escapeHtml(clean))) + suffix;
    }
    return token;
  });

  // Restore placeholders (handle nested placeholders if any)
  let restored = working;
  for (let pass = 0; pass < 3; pass++) {
    if (!restored.includes('\u0000MEMPKB_')) {
      break;
    }
    restored = restored.replace(/\u0000MEMPKB_(\d+)_\u0000/g, (_m, idxStr) => {
      return placeholders[Number(idxStr)] ?? '';
    });
  }

  return restored;
}

/**
 * Fallback lightweight Markdown-to-HTML converter when no external `markdown-it`
 * instance is supplied (e.g. direct unit testing).
 */
function basicMarkdownToHtml(markdown: string): string {
  let html = markdown.replace(/\r\n/g, '\n');
  // Inline code
  html = html.replace(/`([^`\n]+)`/g, (_m, code) => `<code>${escapeHtml(code)}</code>`);
  // Standard markdown links [label](target)
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_m, label, target) => {
    return `<a href="${escapeHtml(target.trim())}">${escapeHtml(label.trim())}</a>`;
  });
  return html;
}

export interface MarkdownItPluginInstance {
  (md?: any): any;
  extendMarkdownIt: (md: any) => any;
  render: (markdown: string, env?: any) => string;
  index: PkbPreviewIndex;
}

/**
 * Create a Markdown-It plugin bound to a `PkbPreviewIndex` or `pkbRoot` directory.
 */
export function createMarkdownItPlugin(
  indexOrRoot?: PkbPreviewIndex | string
): MarkdownItPluginInstance {
  const resolveIndex = (): PkbPreviewIndex => {
    if (indexOrRoot instanceof PkbPreviewIndex) {
      return indexOrRoot;
    }
    if (typeof indexOrRoot === 'string') {
      return new PkbPreviewIndex(indexOrRoot);
    }
    return getPreviewIndex();
  };

  const extendMarkdownIt = (md: any): any => {
    if (!md) {
      return md;
    }
    const origRender =
      typeof md.render === 'function'
        ? md.render.bind(md)
        : (src: string) => basicMarkdownToHtml(src);

    md.render = (src: string, env?: any): string => {
      const idx = resolveIndex();
      const rawHtml = origRender(src, env);
      return transformPreviewHtml(rawHtml, idx);
    };

    if (typeof md.renderInline === 'function') {
      const origRenderInline = md.renderInline.bind(md);
      md.renderInline = (src: string, env?: any): string => {
        const idx = resolveIndex();
        const rawHtml = origRenderInline(src, env);
        return transformPreviewHtml(rawHtml, idx);
      };
    }

    return md;
  };

  const pluginFn = ((md?: any) => {
    if (md) {
      return extendMarkdownIt(md);
    }
    return pluginFn;
  }) as MarkdownItPluginInstance;

  pluginFn.extendMarkdownIt = extendMarkdownIt;
  pluginFn.render = (markdown: string): string => {
    const idx = resolveIndex();
    const rawHtml = basicMarkdownToHtml(markdown);
    return transformPreviewHtml(rawHtml, idx);
  };
  Object.defineProperty(pluginFn, 'index', {
    get: () => resolveIndex(),
  });

  return pluginFn;
}

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

  try {
    logInfo(`Opening referenced PKB file in permanent tab (preview: false): ${uri.fsPath || uri.toString()}`);
    const doc = await workspace.openTextDocument(uri);
    const editor = await window.showTextDocument(doc, {
      preview: options.preview ?? false,
      viewColumn: options.viewColumn ?? vscode.ViewColumn.Active,
    });
    return editor;
  } catch (err) {
    logError(`Failed to open referenced file (${uri.fsPath || uri.toString()})`, err);
    throw err;
  }
}

let isPinningPreviewTab = false;

/**
 * Ensure PKB files opened from Markdown Preview links are pinned as permanent editor tabs (`preview: false`).
 */
export async function handleActiveEditorChanged(
  editor: vscode.TextEditor | undefined,
  index: PkbPreviewIndex = getPreviewIndex(),
  commands: typeof vscode.commands = vscode.commands,
  workspace: typeof vscode.workspace = vscode.workspace,
  window: typeof vscode.window = vscode.window
): Promise<void> {
  if (!editor || !editor.document || isPinningPreviewTab) {
    return;
  }
  const uri = editor.document.uri;
  const fsPath = uri?.fsPath || '';
  if (!fsPath || !fsPath.endsWith('.md')) {
    return;
  }
  if (!index.hasPath(fsPath)) {
    return;
  }

  isPinningPreviewTab = true;
  try {
    logDebug(`Pinning PKB document opened in editor as permanent tab: ${fsPath}`);
    if (typeof commands?.executeCommand === 'function') {
      await commands.executeCommand('workbench.action.keepEditor');
    }
    await openReferencedFile(
      uri,
      { preview: false, viewColumn: editor.viewColumn ?? vscode.ViewColumn.Active },
      workspace,
      window
    );
  } catch (err) {
    logError(`Failed pinning PKB editor tab for ${fsPath}`, err);
  } finally {
    isPinningPreviewTab = false;
  }
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
    const start = match.index;
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

export function getExtensionVersion(context: vscode.ExtensionContext): string {
  try {
    if (context.extension?.packageJSON?.version) {
      return context.extension.packageJSON.version;
    }
    const pkgPath = path.resolve(context.extensionPath || __dirname, '..', 'package.json');
    if (fs.existsSync(pkgPath)) {
      const data = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
      if (data.version) {
        return data.version;
      }
    }
  } catch {
    // fallback
  }
  return '0.1.3';
}

function updateStatusBar(): void {
  if (!statusBarItem) {
    return;
  }
  const nodeCount = previewIndex?.size ?? 0;
  if (serverStatus === 'starting') {
    statusBarItem.text = `$(sync~spin) Mem PKB v${currentExtVersion}`;
    statusBarItem.tooltip = `Mem PKB Language Server v${currentExtVersion} (Starting...)\nBinary: ${currentExecutable}\nPKB Root: ${currentPkbRoot}`;
  } else if (serverStatus === 'running') {
    statusBarItem.text = `$(plug) Mem PKB v${currentExtVersion}`;
    statusBarItem.tooltip = `Mem PKB Language Server v${currentExtVersion} (Running)\nBinary: ${currentExecutable}\nPKB Root: ${currentPkbRoot}\nIndexed Nodes: ${nodeCount}\nClick for status & logs`;
  } else {
    statusBarItem.text = `$(error) Mem PKB v${currentExtVersion} (Error)`;
    statusBarItem.tooltip = `Mem PKB Language Server v${currentExtVersion} (${serverStatus})\nBinary: ${currentExecutable}\nPKB Root: ${currentPkbRoot}\nLast Error: ${lastServerError || 'Server stopped'}\nClick for diagnostics & logs`;
  }
}

async function promptErrorWithLogs(message: string): Promise<void> {
  const selection = await vscode.window.showErrorMessage(
    message,
    'Show Logs',
    'Restart Server'
  );
  if (selection === 'Show Logs') {
    getOutputChannel().show(true);
  } else if (selection === 'Restart Server') {
    await vscode.commands.executeCommand('mem.restartServer');
  }
}

export function activate(context: vscode.ExtensionContext): { extendMarkdownIt: (md: any) => any } {
  currentExtVersion = getExtensionVersion(context);
  const outChannel = getOutputChannel();
  const traceChannel = getTraceOutputChannel();
  context.subscriptions.push(outChannel, traceChannel);

  logInfo(`Activating Mem PKB Language Server extension v${currentExtVersion}`);

  const config = vscode.workspace.getConfiguration('mem.lsp');
  const configuredExec = config.get<string>('executable') || 'pkb';
  currentExecutable = resolvePkbExecutable(configuredExec, logInfo);

  const configuredRoot = config.get<string>('pkbRoot') || '';
  currentPkbRoot = resolvePkbRoot(
    configuredRoot,
    vscode.workspace.workspaceFolders,
    process.env,
    logInfo
  );

  // Initialize synchronous PKB preview index for Markdown Preview windows
  previewIndex = reloadIndex(currentPkbRoot);

  // Status bar showing active version and status
  if (typeof vscode.window.createStatusBarItem === 'function') {
    statusBarItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Right,
      100
    );
    serverStatus = 'starting';
    updateStatusBar();
    statusBarItem.command = 'mem.showVersion';
    statusBarItem.show();
    context.subscriptions.push(statusBarItem);
  }

  const watcher = vscode.workspace.createFileSystemWatcher('**/*.md');
  if (typeof watcher.onDidChange === 'function') {
    context.subscriptions.push(
      watcher.onDidChange((uri: any) => {
        logDebug(`Markdown file changed: ${uri?.fsPath || ''}; refreshing preview index`);
        reloadIndex(currentPkbRoot);
      })
    );
  }
  if (typeof watcher.onDidCreate === 'function') {
    context.subscriptions.push(
      watcher.onDidCreate((uri: any) => {
        logDebug(`Markdown file created: ${uri?.fsPath || ''}; refreshing preview index`);
        reloadIndex(currentPkbRoot);
      })
    );
  }
  if (typeof watcher.onDidDelete === 'function') {
    context.subscriptions.push(
      watcher.onDidDelete((uri: any) => {
        logDebug(`Markdown file deleted: ${uri?.fsPath || ''}; refreshing preview index`);
        reloadIndex(currentPkbRoot);
      })
    );
  }
  context.subscriptions.push(watcher);

  // Server options: launch `pkb lsp --root <pkbRoot>` over stdio
  const serverOptions: ServerOptions = {
    run: {
      command: currentExecutable,
      args: ['lsp', '--root', currentPkbRoot],
      transport: TransportKind.stdio,
    },
    debug: {
      command: currentExecutable,
      args: ['lsp', '--root', currentPkbRoot],
      transport: TransportKind.stdio,
    },
  };

  // Client options: monitor markdown documents and attach output/trace channels & error handlers
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'markdown' },
      { scheme: 'untitled', language: 'markdown' },
    ],
    outputChannel: outChannel as any,
    traceOutputChannel: traceChannel as any,
    synchronize: {
      configurationSection: 'mem.lsp',
      fileEvents: watcher,
    },
    errorHandler: {
      error: (error: Error, message: any, count: number | undefined): ErrorHandlerResult => {
        const errMsg = `LSP transport error (count=${count ?? 1}, msg=${message ? JSON.stringify(message) : 'none'})`;
        lastServerError = `${error.message}`;
        logError(errMsg, error);
        serverStatus = 'error';
        updateStatusBar();
        return { action: ErrorAction.Continue };
      },
      closed: (): CloseHandlerResult => {
        const closeMsg = `Mem PKB LSP server connection closed unexpectedly (binary: ${currentExecutable}, pkbRoot: ${currentPkbRoot})`;
        lastServerError = closeMsg;
        logError(closeMsg);
        serverStatus = 'error';
        updateStatusBar();
        void promptErrorWithLogs(closeMsg);
        return { action: CloseAction.DoNotRestart };
      },
    },
    initializationFailedHandler: (error: Error | any): boolean => {
      const initMsg = `Mem PKB LSP initialization failed: ${error instanceof Error ? error.message : String(error)}`;
      lastServerError = initMsg;
      logError(initMsg, error);
      serverStatus = 'error';
      updateStatusBar();
      void promptErrorWithLogs(initMsg);
      return false;
    },
  };

  logInfo(`Starting LanguageClient with binary="${currentExecutable}" args=["lsp", "--root", "${currentPkbRoot}"]`);
  client = new LanguageClient(
    'mem-lsp',
    'Mem PKB Language Server',
    serverOptions,
    clientOptions
  );

  if (typeof (client as any).onDidChangeState === 'function') {
    const stateSub = (client as any).onDidChangeState((event: { oldState: State; newState: State }) => {
      logInfo(`LSP client state transition: ${event.oldState} -> ${event.newState}`);
      if (event.newState === State.Starting) {
        serverStatus = 'starting';
      } else if (event.newState === State.Running) {
        serverStatus = 'running';
        lastServerError = undefined;
      } else if (event.newState === State.Stopped) {
        if (serverStatus !== 'error') {
          serverStatus = 'stopped';
        }
      }
      updateStatusBar();
    });
    if (stateSub && typeof stateSub.dispose === 'function') {
      context.subscriptions.push(stateSub);
    }
  }

  Promise.resolve(client.start())
    .then(() => {
      serverStatus = 'running';
      lastServerError = undefined;
      logInfo(`Mem PKB LSP server started (pkbRoot=${currentPkbRoot})`);
      updateStatusBar();
    })
    .catch((err: unknown) => {
      lastServerError = err instanceof Error ? err.message : String(err);
      serverStatus = 'error';
      logError(`Failed to start Mem PKB LSP server (binary=${currentExecutable}, pkbRoot=${currentPkbRoot})`, err);
      updateStatusBar();
      void promptErrorWithLogs(`Failed to start Mem PKB LSP server: ${lastServerError}`);
    });

  // Pin PKB files opened via Markdown Preview links as permanent editor tabs (preview: false)
  if (typeof vscode.window.onDidChangeActiveTextEditor === 'function') {
    const editorSub = vscode.window.onDidChangeActiveTextEditor((editor) => {
      void handleActiveEditorChanged(editor, getPreviewIndex());
    });
    if (editorSub && typeof editorSub.dispose === 'function') {
      context.subscriptions.push(editorSub);
    }
  }

  // Register command to reveal the Mem PKB LSP OutputChannel
  const showLogsCmd = vscode.commands.registerCommand('mem.showLogs', () => {
    logInfo('Showing Mem PKB LSP logs OutputChannel');
    getOutputChannel().show(true);
  });

  // Register command to open referenced file in a new tab
  const openCmd = vscode.commands.registerCommand(
    'mem.openReference',
    async (target: string | vscode.Uri) => {
      if (!target) {
        const msg = 'mem.openReference called without a target URI or path';
        logWarn(msg);
        await promptErrorWithLogs(msg);
        return undefined;
      }
      try {
        // Also support resolving a PKB ID via previewIndex if target is not a direct path
        let resolvedTarget: string | vscode.Uri = target;
        if (typeof target === 'string' && !target.startsWith('file://') && !fs.existsSync(target)) {
          const node = getPreviewIndex().resolve(target);
          if (node) {
            resolvedTarget = node.absPath;
          }
        }
        return await openReferencedFile(resolvedTarget, {
          preview: false,
          viewColumn: vscode.ViewColumn.Active,
        });
      } catch (err) {
        const msg = `Failed to open PKB reference (${String(target)}): ${err instanceof Error ? err.message : String(err)}`;
        logError(msg, err);
        await promptErrorWithLogs(msg);
        return undefined;
      }
    }
  );

  // Register command to open reference under active cursor
  const openAtCursorCmd = vscode.commands.registerCommand(
    'mem.openReferenceAtCursor',
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        const msg = 'No active markdown editor to open reference at cursor.';
        logWarn(msg);
        return;
      }

      const pos = editor.selection.active;
      const docUri = editor.document.uri.toString();

      try {
        let targetUri: string | undefined;

        if (client) {
          const def: any = await client.sendRequest('textDocument/definition', {
            textDocument: { uri: docUri },
            position: { line: pos.line, character: pos.character },
          });

          if (def) {
            targetUri = Array.isArray(def)
              ? def[0]?.uri || def[0]?.targetUri
              : def.uri || def.targetUri;
          }
        }

        // Fallback to local extraction + PkbPreviewIndex if LSP returned no definition
        if (!targetUri && typeof editor.document.lineAt === 'function') {
          const lineText = editor.document.lineAt(pos.line)?.text || '';
          const extracted = extractReferenceAtPosition(lineText, pos.character);
          if (extracted) {
            const node = getPreviewIndex().resolve(extracted.target);
            if (node) {
              targetUri = `file://${node.absPath}`;
            }
          }
        }

        if (targetUri) {
          await openReferencedFile(targetUri, {
            preview: false,
            viewColumn: vscode.ViewColumn.Active,
          });
        } else {
          const msg = `No PKB reference found at cursor (${docUri}:${pos.line}:${pos.character})`;
          logWarn(msg);
          const action = await vscode.window.showErrorMessage(msg, 'Show Logs');
          if (action === 'Show Logs') {
            getOutputChannel().show(true);
          }
        }
      } catch (err) {
        const msg = `Failed to open reference at cursor: ${err instanceof Error ? err.message : String(err)}`;
        logError(msg, err);
        const action = await vscode.window.showErrorMessage(msg, 'Show Logs');
        if (action === 'Show Logs') {
          getOutputChannel().show(true);
        }
      }
    }
  );

  // Register command to restart LSP server
  const restartCmd = vscode.commands.registerCommand(
    'mem.restartServer',
    async () => {
      logInfo('Restarting Mem PKB Language Server...');
      reloadIndex(currentPkbRoot);
      if (client) {
        serverStatus = 'starting';
        updateStatusBar();
        try {
          await client.stop();
          await Promise.resolve(client.start());
          serverStatus = 'running';
          lastServerError = undefined;
          updateStatusBar();
          logInfo(`Mem PKB Language Server (v${currentExtVersion}) restarted successfully.`);
          vscode.window.showInformationMessage(
            `Mem PKB Language Server (v${currentExtVersion}) restarted.`
          );
        } catch (err) {
          lastServerError = err instanceof Error ? err.message : String(err);
          serverStatus = 'error';
          updateStatusBar();
          logError('Failed restarting Mem PKB Language Server', err);
          await promptErrorWithLogs(`Failed to restart Mem PKB Language Server: ${lastServerError}`);
        }
      }
    }
  );

  // Register command to display version & diagnostic status
  const showVersionCmd = vscode.commands.registerCommand(
    'mem.showVersion',
    async () => {
      let binVersion = 'unknown';
      try {
        const { execSync } = require('child_process');
        binVersion = execSync(`"${currentExecutable}" --version`, { encoding: 'utf8' }).trim();
      } catch (e) {
        binVersion = `error running ${currentExecutable}: ${e instanceof Error ? e.message : String(e)}`;
        logWarn(`Version check failed for binary ${currentExecutable}: ${binVersion}`);
      }

      let lspStatusDetails = '';
      if (client && serverStatus === 'running') {
        try {
          const statusResp: any = await client.sendRequest('mem/status', {});
          if (statusResp && typeof statusResp === 'object') {
            lspStatusDetails = `\nLSP Node Count: ${statusResp.nodeCount ?? statusResp.nodes ?? 'n/a'}`;
          }
        } catch {
          // Server may not implement custom mem/status yet
        }
      }

      const rootExists = fs.existsSync(currentPkbRoot);
      const indexedNodes = previewIndex?.size ?? 0;
      const msg =
        `Mem PKB Extension: v${currentExtVersion}\n` +
        `Server Status: ${serverStatus}\n` +
        `LSP Binary: ${binVersion}\n` +
        `Binary Path: ${currentExecutable}\n` +
        `PKB Root: ${currentPkbRoot} (exists: ${rootExists})\n` +
        `Preview Index Nodes: ${indexedNodes}` +
        `${lspStatusDetails}\n` +
        `Last Error: ${lastServerError || 'none'}`;

      logInfo(`Status report requested:\n${msg}`);

      const action = await vscode.window.showInformationMessage(
        msg,
        { modal: false },
        'Show Logs',
        'Restart Server',
        'Copy Info'
      );
      if (action === 'Show Logs') {
        getOutputChannel().show(true);
      } else if (action === 'Restart Server') {
        await vscode.commands.executeCommand('mem.restartServer');
      } else if (action === 'Copy Info') {
        if (vscode.env?.clipboard?.writeText) {
          await vscode.env.clipboard.writeText(msg);
          vscode.window.showInformationMessage('Version info copied to clipboard.');
        }
      }
    }
  );

  context.subscriptions.push(
    showLogsCmd,
    openCmd,
    openAtCursorCmd,
    restartCmd,
    showVersionCmd
  );

  const mdPlugin = createMarkdownItPlugin(previewIndex);
  return {
    extendMarkdownIt(md: any) {
      logDebug('Extending Markdown-It instance for Markdown Preview window');
      return mdPlugin.extendMarkdownIt(md);
    },
  };
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}
