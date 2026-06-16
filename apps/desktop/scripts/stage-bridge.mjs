// I1 — stage the Rust napi cdylib for packaging.
//
// electron-builder ships `apps/desktop/build/bridge/<lib>` as an
// UNPACKED extraResource (native libs can't be dlopen'd from inside an
// asar). This script builds the bridge in release and copies the
// platform-correct cdylib into that staging directory, so the packaging
// scripts can stay a simple `stage-bridge && electron-builder`.
//
// Env knobs:
//   * KCREATE_SKIP_BRIDGE_BUILD=1 — reuse an already-built cdylib
//     (skips `cargo build`); errors if the artifact is missing.
//   * KCREATE_BRIDGE_SRC=<path>   — copy from an explicit path instead
//     of `target/release/<lib>` (e.g. a cross-compiled artifact in CI).

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "..", "..");

/** Platform-specific cdylib filename produced by `cargo build`. */
function bridgeLibName(platform = process.platform) {
  if (platform === "win32") return "kcreate_bridge.dll";
  if (platform === "darwin") return "libkcreate_bridge.dylib";
  return "libkcreate_bridge.so";
}

const libName = bridgeLibName();
const stageDir = join(appDir, "build", "bridge");
const destPath = join(stageDir, libName);

if (process.env["KCREATE_SKIP_BRIDGE_BUILD"] !== "1") {
  console.log("[stage-bridge] cargo build --release -p kcreate_bridge");
  const res = spawnSync(
    "cargo",
    ["build", "--release", "-p", "kcreate_bridge"],
    { cwd: repoRoot, stdio: "inherit" },
  );
  if (res.status !== 0) {
    console.error("[stage-bridge] cargo build failed");
    process.exit(res.status ?? 1);
  }
}

const srcPath =
  process.env["KCREATE_BRIDGE_SRC"] ??
  join(repoRoot, "target", "release", libName);

if (!existsSync(srcPath)) {
  console.error(
    `[stage-bridge] cdylib not found at ${srcPath}. ` +
      "Build it first (cargo build --release -p kcreate_bridge) or set " +
      "KCREATE_BRIDGE_SRC.",
  );
  process.exit(1);
}

// Replace any stale artifact so we never package a previous build.
rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });
copyFileSync(srcPath, destPath);

const { size } = statSync(destPath);
console.log(
  `[stage-bridge] staged ${libName} (${(size / 1024 / 1024).toFixed(1)} MB) -> ${destPath}`,
);
