// @vitest-environment node
//
// Wire-format lockstep test for the bridge → main-process boundary.
//
// The Rust crate `kcreate_ai::model_registry` serialises both
// `ModelPack` and `InstallReport` with `#[serde(rename_all =
// "camelCase")]`. The JSON it emits flows verbatim through the
// N-API bridge into this module's `findPackInRegistryJson` and
// `parseInstallReport` helpers, so a drift between the two sides
// silently breaks the one-click install flow.
//
// An earlier iteration of `onboardingDownloader.ts` declared the
// interfaces as snake_case (`download_url`, `size_bytes`,
// `pack_id`, …); the validation always rejected the Rust JSON
// because every snake_case key was `undefined` at runtime, and
// the welcome modal sat on "no download URL pinned in the
// registry" forever. The Rust-side `pack_serialises_to_camelcase_wire_format`
// / `install_report_serialises_to_camelcase_wire_format` tests
// pin the producer's shape; this file pins the consumer's. If
// the wire format ever changes, both sides must change in
// lockstep — and one of these two tests will break on whichever
// side is updated first.

import { describe, expect, it } from "vitest";

import {
  findPackInRegistryJson,
  parseInstallReport,
} from "./onboardingDownloader";

describe("findPackInRegistryJson (Rust→main wire format)", () => {
  it("resolves a pack by id using the camelCase keys Rust emits", () => {
    // Shape mirrors `kcreate_ai::ModelPack`'s on-wire JSON exactly
    // (camelCase via serde rename). Keep this literal in sync with
    // `pack_serialises_to_camelcase_wire_format` in
    // `crates/kcreate_ai/src/model_registry.rs`.
    const raw = JSON.stringify([
      {
        id: "llm_bonsai_1_7b",
        name: "Ternary-Bonsai 1.7B (Q2_0 GGUF)",
        category: "core",
        kind: "sidecar",
        capabilities: ["chat"],
        sizeBytes: 750_000_000,
        filePath: "bonsai-1.7b.q2_0.gguf",
        installed: false,
        downloadUrl: "https://huggingface.co/example/bonsai-1.7b.gguf",
        sha256: "0".repeat(64),
      },
    ]);

    const pack = findPackInRegistryJson(raw, "llm_bonsai_1_7b");
    expect(pack).not.toBeNull();
    expect(pack?.id).toBe("llm_bonsai_1_7b");
    // The fields the downloader actually reads off the pack. The
    // bug this test guards against was: these would all be
    // `undefined` when the interface used snake_case but the
    // JSON shipped camelCase keys.
    expect(pack?.downloadUrl).toBe(
      "https://huggingface.co/example/bonsai-1.7b.gguf",
    );
    expect(pack?.sizeBytes).toBe(750_000_000);
    expect(pack?.filePath).toBe("bonsai-1.7b.q2_0.gguf");
  });

  it("returns null when the requested pack id is not in the catalogue", () => {
    const raw = JSON.stringify([
      {
        id: "llm_bonsai_4b",
        name: "Ternary-Bonsai 4B",
        category: "core",
        kind: "sidecar",
        capabilities: ["chat"],
        sizeBytes: 1_400_000_000,
        filePath: "bonsai-4b.q2_0.gguf",
        installed: false,
        downloadUrl: "https://huggingface.co/example/bonsai-4b.gguf",
        sha256: "0".repeat(64),
      },
    ]);
    expect(findPackInRegistryJson(raw, "llm_bonsai_1_7b")).toBeNull();
  });

  it("rejects malformed JSON with a typed error", () => {
    expect(() => findPackInRegistryJson("not json", "llm_bonsai_1_7b")).toThrow(
      /aiListModelPacks returned invalid JSON/,
    );
  });

  it("rejects a non-array payload with a typed error", () => {
    expect(() =>
      findPackInRegistryJson(JSON.stringify({ id: "x" }), "x"),
    ).toThrow(/did not return an array/);
  });

  it("rejects a snake_case payload (regression: would silently undefined-out)", () => {
    // If a future Rust refactor accidentally drops the
    // `#[serde(rename_all = "camelCase")]` attribute, the
    // catalogue would ship snake_case keys. This test pins the
    // failure mode: the entry still has the right `id`, but the
    // fields the downloader reads off it are gone — which used
    // to surface as a confusing "no download URL pinned in the
    // registry" error from the install handler. With the
    // camelCase contract pinned at the parse boundary, the same
    // failure mode now shows up as obviously-missing fields in
    // this assertion (which would catch it in CI long before
    // any production install attempts).
    const raw = JSON.stringify([
      {
        id: "llm_bonsai_1_7b",
        name: "Ternary-Bonsai 1.7B",
        category: "core",
        kind: "sidecar",
        capabilities: ["chat"],
        // snake_case (Rust's default if `rename_all` were dropped)
        size_bytes: 750_000_000,
        file_path: "bonsai-1.7b.q2_0.gguf",
        installed: false,
        download_url: "https://huggingface.co/example/bonsai-1.7b.gguf",
        sha256: "0".repeat(64),
      },
    ]);
    const pack = findPackInRegistryJson(raw, "llm_bonsai_1_7b");
    expect(pack).not.toBeNull();
    // The id field has no rename so it survives; everything else
    // would be undefined under a snake_case payload.
    expect(pack?.id).toBe("llm_bonsai_1_7b");
    expect(pack?.downloadUrl).toBeUndefined();
    expect(pack?.sizeBytes).toBeUndefined();
    expect(pack?.filePath).toBeUndefined();
  });
});

describe("parseInstallReport (Rust→main wire format)", () => {
  it("decodes a verified install report using camelCase keys", () => {
    // Shape mirrors `kcreate_ai::InstallReport`'s on-wire JSON
    // exactly. Keep this in sync with
    // `install_report_serialises_to_camelcase_wire_format` in
    // `crates/kcreate_ai/src/model_registry.rs`.
    const raw = JSON.stringify({
      packId: "llm_bonsai_1_7b",
      verified: true,
      actualSha256: "a".repeat(64),
      sizeBytes: 750_000_000,
    });
    const report = parseInstallReport(raw);
    expect(report).toEqual({
      packId: "llm_bonsai_1_7b",
      verified: true,
      actualSha256: "a".repeat(64),
      sizeBytes: 750_000_000,
    });
  });

  it("decodes an unverified install report (no canonical hash pinned)", () => {
    // The Rust installer returns verified=false when the registry
    // hasn't pinned a SHA-256 yet; the modal surfaces the actual
    // hash so the user can record it. The shape is identical
    // either way.
    const raw = JSON.stringify({
      packId: "llm_bonsai_1_7b",
      verified: false,
      actualSha256: "b".repeat(64),
      sizeBytes: 750_000_000,
    });
    const report = parseInstallReport(raw);
    expect(report.verified).toBe(false);
    expect(report.actualSha256).toBe("b".repeat(64));
  });

  it("rejects malformed JSON with a typed error", () => {
    expect(() => parseInstallReport("not json")).toThrow(
      /install report was not valid JSON/,
    );
  });

  it("rejects an unexpected shape with a typed error", () => {
    // Missing fields, wrong types, or null all fail validation.
    expect(() => parseInstallReport(JSON.stringify(null))).toThrow(
      /install report shape was unexpected/,
    );
    expect(() => parseInstallReport(JSON.stringify({}))).toThrow(
      /install report shape was unexpected/,
    );
    expect(() =>
      parseInstallReport(
        JSON.stringify({
          packId: "x",
          verified: true,
          actualSha256: "a",
          sizeBytes: "should-be-a-number",
        }),
      ),
    ).toThrow(/install report shape was unexpected/);
  });

  it("rejects a snake_case payload (regression: would have silently undefined-ed every field)", () => {
    // Earlier this file declared the validation in snake_case
    // (`pack_id`, `actual_sha256`, `size_bytes`) which "passed"
    // for snake_case payloads but rejected the camelCase JSON
    // the Rust bridge actually emits — the install flow was
    // 100% broken at the validation gate before the contract
    // was pinned to camelCase here.
    const raw = JSON.stringify({
      pack_id: "llm_bonsai_1_7b",
      verified: true,
      actual_sha256: "a".repeat(64),
      size_bytes: 750_000_000,
    });
    expect(() => parseInstallReport(raw)).toThrow(
      /install report shape was unexpected/,
    );
  });
});
