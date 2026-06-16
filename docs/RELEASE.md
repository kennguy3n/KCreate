# Releasing KCreate

KCreate ships as a self-contained desktop app: the Electron shell, the
Vite-built renderer, and the Rust core (the napi `kcreate_bridge` cdylib)
are packaged into one installer per platform, with auto-update wired in.
This document covers how the packaging works, how to cut a release, and
how signing / notarization are configured.

KCreate stays **local-first**: the editing path never touches the network.
Auto-update is a JS-only, main-process, opt-in affordance — it is the only
part of the app that talks to a server, and only when the user clicks
"Check for updates".

## Artifacts

| Platform | Targets | Auto-update metadata |
| -------- | ------- | -------------------- |
| Linux | `*.AppImage`, `*.deb` | `latest-linux.yml` |
| Windows | `*.exe` (NSIS) | `latest.yml` |
| macOS | `*.dmg`, `*.zip` | `latest-mac.yml` |

The `.zip` on macOS is required by electron-updater (the `.dmg` is the
human download; the updater consumes the `.zip`).

## How packaging is wired

Configuration lives in [`apps/desktop/electron-builder.yml`](../apps/desktop/electron-builder.yml).

1. **JS is pre-bundled.** `pnpm --filter @kcreate/desktop build` runs
   `build.mjs` (esbuild → `main/dist`, `preload/dist`) and
   `vite build` (→ `renderer/dist`). Everything the app imports at runtime
   — including `electron-updater` — is inlined, so the packaged app ships
   **no `node_modules`**. `files` in the config is therefore an explicit
   allow-list of just those build outputs plus `package.json`. This also
   sidesteps pnpm's symlinked store, which electron-builder cannot trace.

2. **The Rust core ships as an unpacked resource.**
   [`scripts/stage-bridge.mjs`](../apps/desktop/scripts/stage-bridge.mjs)
   builds `cargo build --release -p kcreate_bridge` and copies the
   platform cdylib (`libkcreate_bridge.so` / `.dylib` /
   `kcreate_bridge.dll`) into `apps/desktop/build/bridge/`. `extraResources`
   maps that to `<resources>/bridge/`. Native libraries **cannot** be
   `dlopen`'d from inside an asar archive, so this must stay unpacked.
   On macOS it builds both `x86_64-apple-darwin` and `aarch64-apple-darwin`
   and merges them with `lipo` into a **universal** cdylib (override the
   set with `KCREATE_BRIDGE_TARGETS`, or point `KCREATE_BRIDGE_SRC` at an
   already-built artifact).

3. **The app finds the cdylib at runtime.**
   [`bridge.ts::bridgeBinaryPath`](../apps/desktop/main/src/bridge.ts)
   resolves it in this precedence:
   - `KCREATE_BRIDGE_PATH` env override (used by the e2e harness), else
   - `process.resourcesPath/bridge/<lib>` when `app.isPackaged`, else
   - `target/<KCREATE_BRIDGE_PROFILE ?? "debug">/<lib>` for a dev checkout.

   `main.ts` threads `{ isPackaged, resourcesPath }` into `loadBridge`, so
   `bridge.ts` never imports `electron` (keeping it unit-testable).

## Cutting a release locally

From the repo root:

```bash
# one platform at a time (host arch)
pnpm --filter @kcreate/desktop package:linux
pnpm --filter @kcreate/desktop package:mac
pnpm --filter @kcreate/desktop package:win

# everything the host can build
pnpm --filter @kcreate/desktop package
```

Each `package:*` script runs `prepackage` (build + stage the bridge) and
then `electron-builder`. Output lands in `apps/desktop/release/`.

> Building a Windows installer requires running on Windows (or Wine);
> building a macOS `.dmg` requires running on macOS. Linux builds the
> AppImage and `.deb` natively. The CI release lane (below) runs all three
> on their native runners.

To bump the version, edit `version` in `apps/desktop/package.json` (the app
version) and `package.json` at the repo root, then tag:

```bash
git tag v0.0.2
git push origin v0.0.2
```

## CI release lane

[`.github/workflows/release.yml`](../.github/workflows/release.yml) builds
the full matrix (ubuntu-22.04, macos-14, windows-2022) on native runners
and uploads the artifacts. It is fully separate from `ci.yml`, so it never
affects the per-PR fast / smoke lanes.

- **Push a `v*` tag** → builds and **publishes** the artifacts + update
  metadata to the matching GitHub release (`--publish always`).
- **Run it manually** (`workflow_dispatch`) → builds always; set the
  `publish` input to `true` to also publish.

## Auto-update

The updater lives entirely in the main process
([`updater.ts`](../apps/desktop/main/src/updater.ts)) and is exposed to the
renderer over the `kcreate/update/*` IPC channels (`getState`, `check`,
`download`, `quitAndInstall`) plus a `kcreate/update/stateChanged`
broadcast. The in-app affordance is the "Check for updates" control in the
home header ([`UpdatePanel.tsx`](../apps/desktop/renderer/src/components/UpdatePanel.tsx)).

Behavior:

- **Opt-in download.** `autoDownload = false` — a check never silently
  pulls a payload. The user clicks "Download update", then "Restart &
  install". Installs also apply on the next quit
  (`autoInstallOnAppQuit = true`).
- **Provider.** Defaults to the GitHub provider baked from the `publish`
  block in `electron-builder.yml`. Override at runtime with a generic
  server via `KCREATE_UPDATE_FEED_URL` (for self-hosted / air-gapped
  mirrors).
- **Disabled in dev.** Unpackaged runs report `status: "disabled"` and
  render a calm read-only state, so development and the e2e smoke harness
  never hit a feed.

### Environment knobs

| Variable | Effect |
| -------- | ------ |
| `KCREATE_UPDATE_DISABLED=1` | Force the updater off even in a packaged build. |
| `KCREATE_UPDATE_FORCE_DEV=1` | Exercise the real update flow from an unpackaged checkout (`forceDevUpdateConfig`). |
| `KCREATE_UPDATE_FEED_URL=<url>` | Point at a generic update server instead of the baked provider. |

### Verifying an update round-trip locally

1. Package `v0.0.1` (`package:linux`) and keep the AppImage.
2. Bump to `v0.0.2`, package again.
3. Serve `apps/desktop/release/` over HTTP (`python3 -m http.server`).
4. Launch the `v0.0.1` app with
   `KCREATE_UPDATE_FORCE_DEV=1 KCREATE_UPDATE_FEED_URL=http://localhost:8000`.
5. Click "Check for updates" → it reads `latest-linux.yml`, offers
   `0.0.2`, downloads it, and installs on restart.

## Code signing & notarization

Signing is driven entirely by environment variables, so no secrets live in
the repo. When they are unset the build still succeeds and emits **unsigned**
artifacts (useful for forks and local testing).

### macOS

Set in the release environment (CI secrets or a local shell):

| Variable | Purpose |
| -------- | ------- |
| `CSC_LINK` | base64 (or path/URL) of the Developer ID Application `.p12`. |
| `CSC_KEY_PASSWORD` | password for that `.p12`. |
| `APPLE_ID` | Apple ID for notarization. |
| `APPLE_APP_SPECIFIC_PASSWORD` | app-specific password for that Apple ID. |
| `APPLE_TEAM_ID` | the Developer Team ID. |

The build uses a hardened runtime with
[`buildResources/entitlements.mac.plist`](../apps/desktop/buildResources/entitlements.mac.plist)
(JIT + unsigned-executable-memory + `disable-library-validation`, the last
of which is required because the app loads the cdylib via `process.dlopen`).
electron-builder signs, notarizes, and staples automatically when the
variables above are present.

> **arm64 / universal.** The macOS config ships both `x64` and `arm64`
> because `stage-bridge.mjs` produces a **universal** cdylib: it builds the
> bridge for `x86_64-apple-darwin` and `aarch64-apple-darwin` and merges
> them with `lipo`, so each arch slice embeds a loadable bridge. The CI
> lane installs both Rust targets and packages `--mac` (both arches). For a
> fast single-arch dev build, set `KCREATE_BRIDGE_TARGETS=<triple>` and
> constrain electron-builder to the matching arch (`--x64` / `--arm64`) so
> you never package an arch whose cdylib slice is missing.

### Windows

| Variable | Purpose |
| -------- | ------- |
| `WIN_CSC_LINK` | base64 (or path) of the code-signing `.pfx`. |
| `WIN_CSC_KEY_PASSWORD` | password for that `.pfx`. |

For an EV certificate on a hardware token (or Azure Trusted Signing),
follow electron-builder's `signtool` / custom-sign documentation; the NSIS
target picks up the signed binaries automatically.

### Linux

AppImage and `.deb` are not signed by default. Distribute the `.deb` via an
apt repository signed with your release GPG key, or attach detached `.sig`
files to the GitHub release if you want downloadable signatures.
