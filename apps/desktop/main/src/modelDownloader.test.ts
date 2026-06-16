// @vitest-environment node
//
// SSRF allow-list tests for the model downloader. The renderer can
// never influence a download URL — the main process resolves it from
// the native registry — but `validateUrl` / `isHostAllowed` are the
// defence-in-depth gate that runs on the initial URL AND on every
// redirect hop. Hugging Face migrated large-file delivery to its Xet
// CDN, so a `huggingface.co/resolve/main/...` URL now 302s to a
// rotating region/shard host under `*.hf.co` (e.g.
// `cas-bridge.xethub.hf.co`, `us.aws.cdn.hf.co`). These tests pin
// that the allow-list accepts genuine HF-owned hosts while still
// rejecting look-alike domains and non-https schemes.

import { describe, expect, it } from "vitest";

import {
  isHostAllowed,
  validateUrl,
  validateOpenExternalUrl,
} from "./modelDownloader";

describe("isHostAllowed (download SSRF allow-list)", () => {
  it("accepts the Hugging Face apex and legacy LFS CDN hosts", () => {
    expect(isHostAllowed("huggingface.co")).toBe(true);
    expect(isHostAllowed("cdn-lfs.huggingface.co")).toBe(true);
    expect(isHostAllowed("cdn-lfs-us-1.huggingface.co")).toBe(true);
  });

  it("accepts HF Xet CDN hosts under *.hf.co (the migration target)", () => {
    expect(isHostAllowed("cas-bridge.xethub.hf.co")).toBe(true);
    expect(isHostAllowed("us.aws.cdn.hf.co")).toBe(true);
    expect(isHostAllowed("eu.aws.cdn.hf.co")).toBe(true);
    expect(isHostAllowed("hf.co")).toBe(true);
  });

  it("is case-insensitive", () => {
    expect(isHostAllowed("HuggingFace.co")).toBe(true);
    expect(isHostAllowed("CAS-Bridge.XetHub.HF.CO")).toBe(true);
  });

  it("rejects look-alike domains that merely contain the suffix", () => {
    // The suffix match is dot-anchored, so these must NOT slip through.
    expect(isHostAllowed("evil-hf.co")).toBe(false);
    expect(isHostAllowed("hf.co.attacker.test")).toBe(false);
    expect(isHostAllowed("huggingface.co.evil.test")).toBe(false);
    expect(isHostAllowed("nothuggingface.co")).toBe(false);
    expect(isHostAllowed("example.com")).toBe(false);
    expect(isHostAllowed("169.254.169.254")).toBe(false);
  });
});

describe("validateUrl", () => {
  it("accepts an https HF resolve URL", () => {
    const u = validateUrl(
      "https://huggingface.co/Comfy-Org/stable-diffusion-v1-5-archive/resolve/main/v1-5-pruned-emaonly-fp16.safetensors",
    );
    expect(u.hostname).toBe("huggingface.co");
  });

  it("accepts a redirected Xet CDN URL (per-hop revalidation)", () => {
    const u = validateUrl(
      "https://cas-bridge.xethub.hf.co/xet-bridge-us/abc/def?X-Amz-Signature=deadbeef",
    );
    expect(u.hostname).toBe("cas-bridge.xethub.hf.co");
  });

  it("rejects non-https schemes", () => {
    expect(() => validateUrl("http://huggingface.co/x")).toThrow(
      /must use https/,
    );
    expect(() => validateUrl("file:///etc/passwd")).toThrow(/must use https/);
  });

  it("rejects hosts off the allow-list", () => {
    expect(() => validateUrl("https://example.com/model.safetensors")).toThrow(
      /not in the download allow-list/,
    );
    expect(() =>
      validateUrl("https://hf.co.attacker.test/model.safetensors"),
    ).toThrow(/not in the download allow-list/);
  });

  it("rejects malformed URLs with a typed error", () => {
    expect(() => validateUrl("not a url")).toThrow(/invalid pack URL/);
  });
});

describe("validateOpenExternalUrl", () => {
  it("returns a normalized string for an allowed URL", () => {
    expect(
      validateOpenExternalUrl("https://huggingface.co/Comfy-Org"),
    ).toBe("https://huggingface.co/Comfy-Org");
  });

  it("throws for a disallowed URL", () => {
    expect(() => validateOpenExternalUrl("https://evil.test")).toThrow(
      /not in the download allow-list/,
    );
  });
});
