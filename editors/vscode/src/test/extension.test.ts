import * as assert from 'assert';
import * as path from 'path';
import * as fs from 'fs';
import { spawn, ChildProcess } from 'child_process';
import {
  extractReferenceAtPosition,
  openReferencedFile,
} from '../extension';

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
    // CRITICAL: preview must be false to guarantee opening in a permanent new tab
    assert.strictEqual(
      mockShowOptions[0].preview,
      false,
      'Opening a referenced file must set preview: false to open in a new editor tab'
    );
  });
});
