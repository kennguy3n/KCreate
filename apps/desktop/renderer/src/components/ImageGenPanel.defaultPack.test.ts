// Unit tests for `pickDefaultGenerationPack` — the selector default that
// must keep Bonsai strictly opt-in (never an automatic default) even when
// the bridge's `recommendedPack()` returns something unexpected.

import { test, expect } from "vitest";

import type { ModelPack } from "../../../shared/scene";
import { pickDefaultGenerationPack } from "./ImageGenPanel";

function pack(id: string, installed: boolean): ModelPack {
  return {
    id,
    name: id,
    category: "generation",
    kind: "sidecar",
    capabilities: ["image_generation"],
    sizeBytes: 1,
    filePath: `${id}.bin`,
    installed,
    downloadUrl: "",
    sha256: "",
  };
}

const SD15 = "image_gen_sd15";
const FLUX = "image_gen_flux_klein_4b";
const BONSAI_MLX = "image_gen_bonsai_mlx_4b";
const BONSAI_GEMLITE = "image_gen_bonsai_gemlite_4b";

test("the recommended pack wins when present, regardless of install state", () => {
  const gens = [
    pack(FLUX, false),
    pack(SD15, false),
    pack(BONSAI_MLX, false),
    pack(BONSAI_GEMLITE, false),
  ];
  expect(pickDefaultGenerationPack(gens, SD15)).toBe(SD15);
});

test("a recommended Bonsai pack is still honoured (explicit recommendation, not auto)", () => {
  // recommendedPack() never returns Bonsai today, but if it ever did the
  // caller asked for it explicitly — only the *fallback* path is gated.
  const gens = [pack(SD15, false), pack(BONSAI_MLX, false)];
  expect(pickDefaultGenerationPack(gens, BONSAI_MLX)).toBe(BONSAI_MLX);
});

test("empty recommendation skips Bonsai and prefers an installed non-Bonsai pack", () => {
  const gens = [
    pack(BONSAI_MLX, true),
    pack(FLUX, false),
    pack(SD15, true),
  ];
  expect(pickDefaultGenerationPack(gens, "")).toBe(SD15);
});

test("empty recommendation never auto-selects an installed Bonsai pack", () => {
  // The exact invariant the review flagged: Bonsai is the only *installed*
  // generation pack, recommendation is empty — we must still NOT pick it.
  const gens = [
    pack(BONSAI_MLX, true),
    pack(BONSAI_GEMLITE, true),
    pack(SD15, false),
    pack(FLUX, false),
  ];
  const picked = pickDefaultGenerationPack(gens, "");
  expect(picked.startsWith("image_gen_bonsai_")).toBe(false);
  expect(picked).toBe(SD15);
});

test("a recommendation absent from the advertised list falls through, skipping Bonsai", () => {
  const gens = [pack(BONSAI_MLX, true), pack(FLUX, false)];
  expect(pickDefaultGenerationPack(gens, "image_gen_stale_id")).toBe(FLUX);
});

test("a lone Bonsai pack is the last-resort default only when nothing else exists", () => {
  const gens = [pack(BONSAI_GEMLITE, true)];
  expect(pickDefaultGenerationPack(gens, "")).toBe(BONSAI_GEMLITE);
});

test("no generation packs yields an empty selection", () => {
  expect(pickDefaultGenerationPack([], "")).toBe("");
});
