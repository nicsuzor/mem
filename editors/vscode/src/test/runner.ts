import { mockVscode } from './mock-vscode';

// Hook into Module._load / require to provide the mock 'vscode' module
const Module = require('module');
const originalRequire = Module.prototype.require;
Module.prototype.require = function (id: string, ...args: any[]) {
  if (id === 'vscode') {
    return mockVscode;
  }
  return originalRequire.apply(this, [id, ...args]);
};

type TestFn = () => void | Promise<void>;
type HookFn = () => void | Promise<void>;

interface TestCase {
  title: string;
  fn: TestFn;
}

interface Suite {
  title: string;
  tests: TestCase[];
  beforeHooks: HookFn[];
  afterHooks: HookFn[];
}

const suites: Suite[] = [];
let currentSuite: Suite | null = null;

(global as any).describe = (title: string, fn: () => void) => {
  const suite: Suite = {
    title,
    tests: [],
    beforeHooks: [],
    afterHooks: [],
  };
  currentSuite = suite;
  suites.push(suite);
  fn();
  currentSuite = null;
};

(global as any).it = (title: string, fn: TestFn) => {
  if (!currentSuite) {
    throw new Error('it() must be called inside describe()');
  }
  currentSuite.tests.push({ title, fn });
};

(global as any).before = (fn: HookFn) => {
  if (!currentSuite) {
    throw new Error('before() must be called inside describe()');
  }
  currentSuite.beforeHooks.push(fn);
};

(global as any).after = (fn: HookFn) => {
  if (!currentSuite) {
    throw new Error('after() must be called inside describe()');
  }
  currentSuite.afterHooks.push(fn);
};

async function main() {
  console.log('--- Running VS Code Extension Test Suite ---');

  // Load test files
  require('./extension.test');
  require('./integration.test');

  let passed = 0;
  let failed = 0;

  for (const suite of suites) {
    console.log(`\nSuite: ${suite.title}`);

    for (const hook of suite.beforeHooks) {
      await hook();
    }

    for (const test of suite.tests) {
      try {
        await test.fn();
        console.log(`  ✓ ${test.title}`);
        passed++;
      } catch (err: any) {
        console.error(`  ✗ ${test.title}`);
        console.error(err);
        failed++;
      }
    }

    for (const hook of suite.afterHooks) {
      await hook();
    }
  }

  console.log(`\n================================`);
  console.log(`Results: ${passed} passed, ${failed} failed`);
  console.log(`================================\n`);

  if (failed > 0) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error('Test runner failure:', err);
  process.exit(1);
});
