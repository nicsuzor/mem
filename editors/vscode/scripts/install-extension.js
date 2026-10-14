const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const rootDir = path.resolve(__dirname, '..');
const pkg = JSON.parse(fs.readFileSync(path.join(rootDir, 'package.json'), 'utf8'));
const version = pkg.version;
const vsixName = `mem-vscode-${version}.vsix`;
const vsixPath = path.join(rootDir, vsixName);

if (!fs.existsSync(vsixPath)) {
  console.log(`[mem-vscode] VSIX file ${vsixName} not found, building...`);
  execSync('npm run package', { cwd: rootDir, stdio: 'inherit' });
}

// Candidate CLI binaries for installing VS Code extensions
const candidates = [
  '/Applications/Antigravity IDE.app/Contents/Resources/app/bin/antigravity-ide',
  'antigravity-ide',
  '/opt/homebrew/bin/code-insiders',
  'code-insiders',
  'code',
];

let installedCount = 0;
for (const cli of candidates) {
  try {
    let resolvedCli = cli;
    if (!cli.startsWith('/') && !fs.existsSync(cli)) {
      try {
        resolvedCli = execSync(`which ${cli}`, { encoding: 'utf8' }).trim();
      } catch {
        continue;
      }
    } else if (!fs.existsSync(cli)) {
      continue;
    }

    console.log(`[mem-vscode] Installing ${vsixName} using ${resolvedCli}...`);
    execSync(`"${resolvedCli}" --install-extension "${vsixPath}" --force`, { stdio: 'inherit' });
    console.log(`[mem-vscode] Successfully installed v${version} via ${path.basename(resolvedCli)}`);
    installedCount++;
  } catch (err) {
    console.warn(`[mem-vscode] Failed to install via ${cli}:`, err instanceof Error ? err.message : String(err));
  }
}

if (installedCount === 0) {
  console.log(`[mem-vscode] Packaged ${vsixName} at ${vsixPath}`);
  console.log(`[mem-vscode] To install manually, run:`);
  console.log(`  antigravity-ide --install-extension "${vsixPath}" --force`);
} else {
  console.log(`[mem-vscode] All done! Reload window in Antigravity IDE (Cmd+Shift+P -> "Developer: Reload Window") to activate v${version}.`);
}
