export class MockPosition {
  constructor(public line: number = 0, public character: number = 0) {}
}

export class MockRange {
  public start: MockPosition;
  public end: MockPosition;

  constructor(
    startOrLine: MockPosition | number = new MockPosition(),
    endOrChar: MockPosition | number = new MockPosition(),
    endLine?: number,
    endChar?: number
  ) {
    if (typeof startOrLine === 'number' && typeof endOrChar === 'number') {
      this.start = new MockPosition(startOrLine, endOrChar);
      this.end = new MockPosition(endLine ?? startOrLine, endChar ?? endOrChar);
    } else {
      this.start = (startOrLine as MockPosition) || new MockPosition();
      this.end = (endOrChar as MockPosition) || new MockPosition();
    }
  }
}

export class MockLocation {
  constructor(public uri: any = null, public range: any = null) {}
}

export class MockHover {
  constructor(public contents: any = null, public range?: any) {}
}

export class MockDocumentLink {
  constructor(public range: any = null, public target?: any, public tooltip?: string) {}
}

export class MockCompletionItem {
  constructor(public label: string = '', public kind?: any) {}
}

export class MockCodeAction {
  constructor(public title: string = '', public kind?: any) {}
}

export class MockCodeLens {
  constructor(public range: any = null, public command?: any) {}
}

export class MockDiagnostic {
  constructor(public range: any = null, public message: string = '', public severity?: any) {}
}

export class MockSnippetString {
  constructor(public value: string = '') {}
}

export class MockMarkdownString {
  constructor(public value: string = '') {}
}

export class MockCancellationError extends Error {
  constructor() {
    super('Canceled');
  }
}

export class MockCallHierarchyItem {
  constructor(public kind: any = 0, public name: string = '', public detail: string = '', public uri: any = null, public range: any = null, public selectionRange: any = null) {}
}

export class MockTypeHierarchyItem {
  constructor(public kind: any = 0, public name: string = '', public detail: string = '', public uri: any = null, public range: any = null, public selectionRange: any = null) {}
}

export class MockSymbolInformation {
  constructor(public name: string = '', public kind: any = 0, public containerName?: string, public location?: any) {}
}

export class MockInlayHint {
  constructor(public position: any = null, public label: any = '', public kind?: any) {}
}

export class MockRelativePattern {
  constructor(public base: any, public pattern: string) {}
}

export class MockDisposable {
  static from(..._disposables: { dispose(): any }[]) {
    return new MockDisposable();
  }
  dispose() {}
}

export class MockEventEmitter {
  private listeners: ((data?: any) => any)[] = [];
  event = (listener: (data?: any) => any) => {
    this.listeners.push(listener);
    return { dispose: () => {} };
  };
  fire(data?: any) {
    for (const l of this.listeners) {
      l(data);
    }
  }
  dispose() {
    this.listeners = [];
  }
}

export class MockOutputChannel {
  public lines: string[] = [];
  public shown: boolean = false;
  public preserveFocusLast?: boolean;
  public logLevel: number = 3;

  constructor(public name: string) {}

  append(value: string): void {
    this.lines.push(value);
  }

  appendLine(value: string): void {
    this.lines.push(value);
  }

  replace(value: string): void {
    this.lines = [value];
  }

  clear(): void {
    this.lines = [];
  }

  show(preserveFocus?: boolean): void {
    this.shown = true;
    this.preserveFocusLast = preserveFocus;
  }

  hide(): void {
    this.shown = false;
  }

  dispose(): void {}

  error(error: string | Error, ..._args: any[]): void {
    const text = error instanceof Error ? (error.message || String(error)) : String(error);
    this.appendLine(`[${new Date().toISOString()}] [ERROR] ${text}`);
  }

  warn(message: string, ..._args: any[]): void {
    this.appendLine(`[${new Date().toISOString()}] [WARN] ${String(message)}`);
  }

  info(message: string, ..._args: any[]): void {
    this.appendLine(`[${new Date().toISOString()}] [INFO] ${String(message)}`);
  }

  debug(message: string, ..._args: any[]): void {
    this.appendLine(`[${new Date().toISOString()}] [DEBUG] ${String(message)}`);
  }

  trace(message: string, ..._args: any[]): void {
    this.appendLine(`[${new Date().toISOString()}] [DEBUG] ${String(message)}`);
  }

  onDidChangeLogLevel(_listener: (level: any) => any): { dispose: () => void } {
    return { dispose: () => {} };
  }
}

export class MockUri {
  scheme: string;
  authority: string;
  path: string;
  query: string;
  fragment: string;
  fsPath: string;

  constructor(fsPath: string) {
    this.fsPath = fsPath;
    this.path = fsPath;
    this.scheme = 'file';
    this.authority = '';
    this.query = '';
    this.fragment = '';
  }

  toString(): string {
    return `file://${this.fsPath}`;
  }

  static file(filePath: string): MockUri {
    return new MockUri(filePath);
  }

  static parse(uriString: string): MockUri {
    const fsPath = uriString.startsWith('file://') ? uriString.slice(7) : uriString;
    return new MockUri(fsPath);
  }
}

export const createdOutputChannels: Map<string, MockOutputChannel> = new Map();
export const registeredCommands: Map<string, (...args: any[]) => any> = new Map();
export const activeEditorListeners: ((editor: any) => any)[] = [];

const baseMock: Record<string, any> = {
  Uri: MockUri,
  Position: MockPosition,
  Range: MockRange,
  Location: MockLocation,
  Hover: MockHover,
  DocumentLink: MockDocumentLink,
  CompletionItem: MockCompletionItem,
  CodeAction: MockCodeAction,
  CodeLens: MockCodeLens,
  Diagnostic: MockDiagnostic,
  SnippetString: MockSnippetString,
  MarkdownString: MockMarkdownString,
  CancellationError: MockCancellationError,
  CallHierarchyItem: MockCallHierarchyItem,
  TypeHierarchyItem: MockTypeHierarchyItem,
  SymbolInformation: MockSymbolInformation,
  InlayHint: MockInlayHint,
  RelativePattern: MockRelativePattern,
  Disposable: MockDisposable,
  EventEmitter: MockEventEmitter,
  version: '1.91.0',
  OutputChannel: MockOutputChannel,
  LogLevel: {
    Off: 0,
    Trace: 1,
    Debug: 2,
    Info: 3,
    Warning: 4,
    Error: 5,
  },
  DiagnosticSeverity: {
    Error: 0,
    Warning: 1,
    Information: 2,
    Hint: 3,
  },
  ViewColumn: {
    Active: 1,
    Beside: 2,
    One: 1,
    Two: 2,
  },
  StatusBarAlignment: {
    Left: 1,
    Right: 2,
  },
  workspace: {
    getConfiguration: (_section?: string) => ({
      get: (_key: string, defaultValue?: any) => defaultValue,
    }),
    workspaceFolders: [] as any[],
    onDidChangeConfiguration: (_listener: any) => ({ dispose: () => {} }),
    onDidOpenTextDocument: (_listener: any) => ({ dispose: () => {} }),
    onDidCloseTextDocument: (_listener: any) => ({ dispose: () => {} }),
    onDidChangeTextDocument: (_listener: any) => ({ dispose: () => {} }),
    onDidSaveTextDocument: (_listener: any) => ({ dispose: () => {} }),
    createFileSystemWatcher: () => ({
      onDidChange: () => ({ dispose: () => {} }),
      onDidCreate: () => ({ dispose: () => {} }),
      onDidDelete: () => ({ dispose: () => {} }),
      dispose: () => {},
    }),
    openTextDocument: async (uri: any) => ({
      uri,
      fileName: uri.fsPath || String(uri),
      getText: () => '',
    }),
  },
  window: {
    activeTextEditor: undefined as any,
    onDidChangeActiveTextEditor: (listener: (editor: any) => any) => {
      activeEditorListeners.push(listener);
      return { dispose: () => {} };
    },
    createOutputChannel: (name: string, _options?: any) => {
      let channel = createdOutputChannels.get(name);
      if (!channel) {
        channel = new MockOutputChannel(name);
        createdOutputChannels.set(name, channel);
      }
      return channel;
    },
    showTextDocument: async (doc: any, options?: any) => ({
      document: doc,
      options,
    }),
    showErrorMessage: async (_msg: string, ..._items: string[]) => undefined,
    showWarningMessage: async (_msg: string, ..._items: string[]) => undefined,
    showInformationMessage: async (_msg: string, ..._actions: any[]) => undefined,
    showQuickPick: async (items: any[], _options?: any) => (Array.isArray(items) ? items[0] : undefined),
    showInputBox: async (_options?: any) => undefined,
    createStatusBarItem: (_alignment?: any, _priority?: number) => ({
      text: '',
      tooltip: '',
      command: '',
      show: () => {},
      hide: () => {},
      dispose: () => {},
    }),
    createTextEditorDecorationType: (options: any) => ({
      key: `dec-${Math.random()}`,
      options,
      dispose: () => {},
    }),
  },
  languages: {
    registerInlayHintsProvider: (_selector: any, _provider: any) => ({ dispose: () => {} }),
    registerCodeLensProvider: (_selector: any, _provider: any) => ({ dispose: () => {} }),
    registerDocumentSemanticTokensProvider: (_selector: any, _provider: any, _legend: any) => ({
      dispose: () => {},
    }),
    registerDocumentLinkProvider: (_selector: any, _provider: any) => ({
      dispose: () => {},
    }),
  },
  commands: {
    registerCommand: (command: string, callback: (...args: any[]) => any) => {
      registeredCommands.set(command, callback);
      return {
        dispose: () => {
          registeredCommands.delete(command);
        },
      };
    },
    executeCommand: async (command: string, ...args: any[]) => {
      const handler = registeredCommands.get(command);
      if (handler) {
        return await handler(...args);
      }
      return undefined;
    },
  },
  env: {
    clipboard: {
      writeText: async (_text: string) => {},
    },
  },
};

export const mockVscode = new Proxy(baseMock, {
  get(target, prop: string) {
    if (prop in target) {
      return target[prop];
    }
    // Return a constructable class function for any unspecified class/symbol
    function GenericConstructor(this: any, ..._args: any[]) {
      return this;
    }
    GenericConstructor.prototype = {};
    return GenericConstructor;
  },
});
