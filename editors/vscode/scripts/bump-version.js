const fs = require('fs');
const path = require('path');

const pkgPath = path.resolve(__dirname, '..', 'package.json');
if (!fs.existsSync(pkgPath)) {
  console.error(`package.json not found at ${pkgPath}`);
  process.exit(1);
}

const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
const currentVersion = pkg.version || '0.1.0';
const parts = currentVersion.split('.').map(n => parseInt(n, 10) || 0);

while (parts.length < 3) {
  parts.push(0);
}

// If an explicit SemVer version is provided as argv[2], use it; otherwise bump
let newVersion;
const arg = process.argv[2];

if (arg && /^\d+\.\d+\.\d+(?:-[\w.]+)?$/.test(arg)) {
  newVersion = arg;
} else {
  const bumpType = arg || 'patch';
  if (bumpType === 'major') {
    parts[0] += 1;
    parts[1] = 0;
    parts[2] = 0;
  } else if (bumpType === 'minor') {
    parts[1] += 1;
    parts[2] = 0;
  } else {
    parts[2] += 1;
  }
  newVersion = parts.join('.');
}
pkg.version = newVersion;
fs.writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + '\n');
console.log(`[mem-vscode] Updated extension version: ${currentVersion} -> ${newVersion}`);

// Also sync package-lock.json if present
const lockPath = path.resolve(__dirname, '..', 'package-lock.json');
if (fs.existsSync(lockPath)) {
  try {
    const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
    lock.version = newVersion;
    if (lock.packages && lock.packages['']) {
      lock.packages[''].version = newVersion;
    }
    fs.writeFileSync(lockPath, JSON.stringify(lock, null, 2) + '\n');
  } catch (err) {
    console.warn(`[mem-vscode] Warning: could not update package-lock.json:`, err);
  }
}
