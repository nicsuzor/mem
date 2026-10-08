export class MockPosition {
  constructor(public line: number = 0, public character: number = 0) {}
}

export class MockRange {
  constructor(public start: MockPosition = new MockPosition(), public end: MockPosition = new MockPosition()) {}
}

export class MockLocation {
  constructor(public uri: any = null, public range: any = null) {}
}

export class MockHover {
  constructor(public contents: any = null, public range?: any) {}
}

export class MockDocumentLink {
  constructor(public range: any = null, public target?: any) {}
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

export class MockDisposable {
  static from(..._disposables: { dispose(): any }[]) {
    return new MockDisposable();
  }
  dispose() {}
}

export class MockEventEmitter {
  event = () => ({ dispose: () => {} });
  fire(_data?: any) {}
  dispose() {}
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
  Disposable: MockDisposable,
  EventEmitter: MockEventEmitter,
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
  workspace: {
    getConfiguration: (_section?: string) => ({
      get: (_key: string, defaultValue?: any) => defaultValue,
    }),
    workspaceFolders: [] as any[],
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
    showTextDocument: async (doc: any, options?: any) => ({
      document: doc,
      options,
    }),
    showErrorMessage: async (_msg: string) => {},
    showInformationMessage: async (_msg: string) => {},
  },
  commands: {
    registerCommand: (_command: string, _callback: (...args: any[]) => any) => ({
      dispose: () => {},
    }),
    executeCommand: async (_command: string, ..._args: any[]) => {},
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
