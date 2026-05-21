//! Local-first invariant check.
//!
//! Phase 0 promises that no network calls happen during ordinary
//! editing. We can't perfectly police that from Rust (an adversarial
//! dependency could open sockets), but we *can* check the dependency
//! tree of the editing-path crates against a deny-list of known
//! networking crates. If a network client ever sneaks in via a
//! transitive dependency, this test fails and forces an explicit
//! review.
//!
//! The deny-list intentionally errs on the side of false positives:
//! anything that opens sockets, drives HTTP, or speaks DNS is in.

use std::process::Command;

const DENIED_CRATES: &[&str] = &[
    "reqwest",
    "hyper",
    "ureq",
    "isahc",
    "curl",
    "tonic",
    "rustls", // TLS implies networking for our purposes
    "native-tls",
    "openssl", // crypto only; we don't link OpenSSL anywhere
    "trust-dns",
    "hickory-dns",
    "trust-dns-resolver",
    "hickory-resolver",
    "rumqttc",
    "lapin",
    "rdkafka",
    // The MCP sidecar (kcreate_mcp) links tiny_http. Per AGENTS.md it
    // must NOT appear in the editing-path tree under default features:
    // kcreate_bridge declares kcreate_mcp behind the optional `mcp`
    // feature so default builds (and this test) keep tiny_http out.
    "tiny_http",
];

/// Crates whose runtime is networking-heavy. We enforce a stronger
/// invariant on these: they must not appear *anywhere* in the editing
/// dependency tree.
const fn editing_path_crates() -> &'static [&'static str] {
    &[
        "kcreate_core",
        "kcreate_storage",
        "kcreate_vector",
        "kcreate_export",
        "kcreate_renderer",
        "kcreate_bridge",
    ]
}

#[test]
fn editing_path_pulls_no_network_crates() {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata must run");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    // We use `cargo tree --prefix none --no-dedupe` per editing crate
    // to enumerate the full dependency closure. `--no-dedupe` ensures
    // we see *every* path so we can't miss a transitive network dep
    // hidden behind dedup.
    for crate_name in editing_path_crates() {
        let tree = Command::new("cargo")
            .args([
                "tree",
                "-p",
                crate_name,
                "--prefix",
                "none",
                "--no-dedupe",
                "--edges",
                "normal,build",
            ])
            .output()
            .expect("cargo tree must run");
        assert!(
            tree.status.success(),
            "cargo tree -p {crate_name} failed: {}",
            String::from_utf8_lossy(&tree.stderr),
        );
        let stdout = String::from_utf8_lossy(&tree.stdout);
        for line in stdout.lines() {
            // Lines look like "name vX.Y.Z" or "name vX.Y.Z (proc-macro)"
            // — strip trailing version + any qualifier.
            let Some(name) = line.split_whitespace().next() else {
                continue;
            };
            for denied in DENIED_CRATES {
                assert_ne!(
                    name, *denied,
                    "{crate_name} pulls in network crate `{denied}` — \
                     local-first contract broken (full tree:\n{stdout}\n)",
                );
            }
        }
    }
}
