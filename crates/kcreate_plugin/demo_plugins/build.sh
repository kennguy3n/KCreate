#!/usr/bin/env bash
#
# Rebuild the two bundled WASM demo plugins from source and re-sign them.
#
# The demo crates are deliberately DETACHED from the workspace (each has an
# empty `[workspace]` table) so they can target `wasm32-unknown-unknown`
# without dragging the host's native deps into a wasm build. This script
# compiles each one, copies the resulting `.wasm` into the bundled registry
# under the filename the manifest's `entry_point` expects, and then runs the
# `sign_bundled` tool to regenerate `manifest.json.sig` + `trusted_keys.json`
# with the development signing key.
#
# Network IS used here for the build toolchain — that is fine. The offline
# guarantee is a *runtime / editing-path* property; this is an offline,
# reproducible build step run by maintainers, never on the editing path.
#
# Usage:
#   rustup target add wasm32-unknown-unknown   # one-time
#   crates/kcreate_plugin/demo_plugins/build.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bundled="$(cd "${here}/../bundled" && pwd)"
target="wasm32-unknown-unknown"

# crate dir : output wasm name : bundled plugin dir : entry_point filename
plugins=(
  "grid_arrange:kcreate_demo_grid_arrange:com.kcreate.demo.grid-arrange:grid_arrange.wasm"
  "palette_apply:kcreate_demo_palette_apply:com.kcreate.demo.palette-apply:palette_apply.wasm"
)

for spec in "${plugins[@]}"; do
  IFS=':' read -r crate_dir lib_name plugin_id entry <<<"${spec}"
  echo ">> building ${crate_dir} -> ${target}"
  (cd "${here}/${crate_dir}" && cargo build --release --target "${target}")
  src="${here}/${crate_dir}/target/${target}/release/${lib_name}.wasm"
  dst="${bundled}/${plugin_id}/${entry}"
  echo ">> copying $(basename "${src}") -> ${plugin_id}/${entry}"
  cp "${src}" "${dst}"
done

echo ">> signing bundled plugins"
cargo run --manifest-path "${here}/sign_bundled/Cargo.toml" -- "${bundled}"

echo ">> done. Bundled artifacts in ${bundled}"
