#!/usr/bin/env node

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const https = require("node:https");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const pkg = require(path.join(root, "package.json"));

function assetName() {
  const exe = process.platform === "win32" ? ".exe" : "";
  return `fp-${process.platform}-${process.arch}${exe}`;
}

function releaseUrl() {
  const base =
    process.env.FLASHPOINT_DOWNLOAD_BASE ||
    `https://github.com/manfad/flashpoint/releases/download/v${pkg.version}`;
  return `${base.replace(/\/$/, "")}/${assetName()}`;
}

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    const request = https.get(url, (response) => {
      if (
        response.statusCode >= 300 &&
        response.statusCode < 400 &&
        response.headers.location
      ) {
        response.resume();
        if (redirects > 5) {
          reject(new Error("too many redirects"));
          return;
        }
        download(response.headers.location, dest, redirects + 1)
          .then(resolve)
          .catch(reject);
        return;
      }

      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download failed with HTTP ${response.statusCode}`));
        return;
      }

      const tmp = `${dest}.tmp`;
      const file = fs.createWriteStream(tmp, { mode: 0o755 });
      response.pipe(file);
      file.on("finish", () => {
        file.close(() => {
          fs.renameSync(tmp, dest);
          if (process.platform !== "win32") fs.chmodSync(dest, 0o755);
          resolve();
        });
      });
      file.on("error", (error) => {
        fs.rmSync(tmp, { force: true });
        reject(error);
      });
    });
    request.on("error", reject);
  });
}

function buildFromSource(dest) {
  const exe = process.platform === "win32" ? ".exe" : "";
  const cargoToml = path.join(root, "Cargo.toml");
  if (!fs.existsSync(cargoToml)) {
    throw new Error("source files are not available for cargo fallback");
  }

  execFileSync("cargo", ["build", "--release", "--locked"], {
    cwd: root,
    stdio: "inherit",
  });

  fs.copyFileSync(path.join(root, "target", "release", `fp${exe}`), dest);
  if (process.platform !== "win32") fs.chmodSync(dest, 0o755);
}

async function main() {
  if (process.env.FLASHPOINT_SKIP_DOWNLOAD === "1") return;

  const dest = path.join(__dirname, "bin", assetName());
  fs.mkdirSync(path.dirname(dest), { recursive: true });

  try {
    await download(releaseUrl(), dest);
    return;
  } catch (downloadError) {
    console.warn(`flashpoint: ${downloadError.message}; building from source`);
  }

  try {
    buildFromSource(dest);
  } catch (buildError) {
    console.error(
      `flashpoint: could not install native binary for ${process.platform}/${process.arch}`
    );
    console.error(buildError.message);
    process.exit(1);
  }
}

main();
