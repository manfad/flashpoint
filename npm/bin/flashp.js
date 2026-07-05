#!/usr/bin/env node

const { spawn } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

function assetName() {
  const exe = process.platform === "win32" ? ".exe" : "";
  return `fp-${process.platform}-${process.arch}${exe}`;
}

const binPath = path.join(__dirname, assetName());

if (!fs.existsSync(binPath)) {
  console.error(
    "flashpoint native binary was not installed. Run `npm rebuild flashp` or reinstall the package."
  );
  process.exit(1);
}

const child = spawn(binPath, process.argv.slice(2), { stdio: "inherit" });

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    if (!child.killed) child.kill(signal);
  });
}

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
