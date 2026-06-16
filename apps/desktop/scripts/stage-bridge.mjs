// I1 — stage the Rust napi cdylib for packaging.
//
// electron-builder ships `apps/desktop/build/bridge/<lib>` as an
// UNPACKED extraResource (native libs can't be dlopen'd from inside an
// asar). This script builds the bridge in release and copies the
// platform-correct cdylib into that staging directory, so the packaging
// scripts can stay a simple `stage-bridge && electron-builder`.
//
// macOS ships a UNIVERSAL (x86_64 + arm64) cdylib, merged with `lipo`.
// electron-builder declares both mac arches (see electron-builder.yml),
// so a single-arch cdylib would leave the non-host slice with a bridge
// it can't load. Building both slices and merging keeps every produced
// .app/.dmg valid regardless of which arch it targets.
//
// Env knobs:
//   * KCREATE_SKIP_BRIDGE_BUILD=1 — reuse already-built cdylib(s)
//     (skips `cargo build`); errors if an artifact is missing.
//   * KCREATE_BRIDGE_SRC=<path>   — copy from an explicit, already-correct
//     artifact (e.g. a pre-merged universal dylib) instead of building.
//   * KCREATE_BRIDGE_TARGETS=<t1,t2,...> — override the Rust target triples
//     to build + merge. Defaults to both apple-darwin triples on macOS and
//     the host default elsewhere. Set a single triple for a fast
//     single-arch dev build (then constrain electron-builder with
//     `--x64` / `--arm64` to match).

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
const skipBuild = process.env["KCREATE_SKIP_BRIDGE_BUILD"] === "1";

/** Run a command, inheriting stdio, and bail out on a non-zero exit. */
function run(cmd, args) {
  const res = spawnSync(cmd, args, { cwd: repoRoot, stdio: "inherit" });
  if (res.status !== 0) {
    console.error(`[stage-bridge] \`${cmd} ${args.join(" ")}\` failed`);
    process.exit(res.status ?? 1);
  }
}

/** Build one slice. An empty triple uses cargo's default (host) target. */
function buildSlice(triple) {
  if (skipBuild) return;
  const args = ["build", "--release", "-p", "kcreate_bridge"];
  if (triple) args.push("--target", triple);
  console.log(`[stage-bridge] cargo ${args.join(" ")}`);
  run("cargo", args);
}

/** Where cargo writes a given triple's release artifact. */
function artifactPath(triple) {
  return triple
    ? join(repoRoot, "target", triple, "release", libName)
    : join(repoRoot, "target", "release", libName);
}

function requireArtifact(p) {
  if (!existsSync(p)) {
    console.error(
      `[stage-bridge] cdylib not found at ${p}. Build it first ` +
        "(cargo build --release -p kcreate_bridge [--target <triple>]) " +
        "or set KCREATE_BRIDGE_SRC.",
    );
    process.exit(1);
  }
  return p;
}

// Default triples to merge: a universal pair on macOS, the host default
// (empty triple ⇒ `target/release/`) elsewhere.
function defaultTargets() {
  if (process.platform === "darwin") {
    return ["x86_64-apple-darwin", "aarch64-apple-darwin"];
  }
  return [""];
}

const targets =
  process.env["KCREATE_BRIDGE_TARGETS"] != null
    ? process.env["KCREATE_BRIDGE_TARGETS"]
        .split(",")
        .map((t) => t.trim())
        .filter((t) => t.length > 0)
    : defaultTargets();

// Replace any stale artifact so we never package a previous build.
rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });

if (process.env["KCREATE_BRIDGE_SRC"]) {
  // Explicit, already-correct artifact (e.g. a pre-merged universal dylib).
  copyFileSync(requireArtifact(process.env["KCREATE_BRIDGE_SRC"]), destPath);
} else if (targets.length <= 1) {
  // Single arch (host or one explicit triple): straight copy.
  const triple = targets[0] ?? "";
  buildSlice(triple);
  copyFileSync(requireArtifact(artifactPath(triple)), destPath);
} else {
  // Multi-arch: build each slice and merge with `lipo` (a macOS tool).
  if (process.platform !== "darwin") {
    console.error(
      "[stage-bridge] multi-target merge (lipo) is only supported on macOS; " +
        `got targets [${targets.join(", ")}] on ${process.platform}. ` +
        "Set KCREATE_BRIDGE_TARGETS to a single triple for this platform.",
    );
    process.exit(1);
  }
  const slices = targets.map((triple) => {
    buildSlice(triple);
    return requireArtifact(artifactPath(triple));
  });
  console.log(
    `[stage-bridge] lipo -create (${targets.join(", ")}) -> ${libName}`,
  );
  run("lipo", ["-create", ...slices, "-output", destPath]);
  // Surface the merged architectures so a broken merge fails loudly.
  run("lipo", ["-info", destPath]);
}

const { size } = statSync(destPath);
console.log(
  `[stage-bridge] staged ${libName} (${(size / 1024 / 1024).toFixed(1)} MB) -> ${destPath}`,
);
