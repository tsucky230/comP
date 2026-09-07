#!/usr/bin/env node

/**
 * Copy the locally built Rust daemon binary to the platform-specific filename
 * that .vscodeignore expects (comp-daemon-win.exe / comp-daemon-linux / comp-daemon-macos).
 *
 * WHY: release.yml builds all 3 platforms in a CI matrix and renames each
 * artifact before `vsce package`. A local `vsce package` only has the current
 * platform's dev binary (comp-daemon / comp-daemon.exe) at
 * daemon/target/release/, which .vscodeignore's negation patterns don't match,
 * so the daemon binary silently gets excluded from the VSIX. This script
 * reproduces the CI rename step for local packaging/testing.
 */

const fs = require('fs');
const path = require('path');

const releaseDir = path.join(__dirname, '..', 'daemon', 'target', 'release');

const PLATFORM_MAP = {
  win32: { src: 'comp-daemon.exe', dest: 'comp-daemon-win.exe' },
  linux: { src: 'comp-daemon', dest: 'comp-daemon-linux' },
  darwin: { src: 'comp-daemon', dest: 'comp-daemon-macos' },
};

function main() {
  const mapping = PLATFORM_MAP[process.platform];
  if (!mapping) {
    console.error(`[comP] Unsupported platform for local packaging: ${process.platform}`);
    process.exit(1);
  }

  const srcPath = path.join(releaseDir, mapping.src);
  const destPath = path.join(releaseDir, mapping.dest);

  if (!fs.existsSync(srcPath)) {
    console.error(`[comP] Daemon binary not found: ${srcPath}`);
    console.error('[comP] Run `npm run daemon:build` first.');
    process.exit(1);
  }

  fs.copyFileSync(srcPath, destPath);
  if (process.platform !== 'win32') {
    fs.chmodSync(destPath, 0o755);
  }

  console.log(`[comP] Copied ${mapping.src} -> ${mapping.dest} for local VSIX packaging`);
}

main();
