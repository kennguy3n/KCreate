// ModelManager tests — in-app model download UX (this workstream).
//
// Pins the behaviours the spec calls for on the renderer side:
//   * a generation pack is hard-gated on `imageGenerationAllowed`
//     — hidden entirely when the tier forbids image generation,
//     surfaced with a Download button when it allows it;
//   * clicking Download routes to `aiModel.downloadModelPack(packId)`
//     (downloads happen only on explicit user action) and, while the
//     download is in flight, renders an accessible progress bar driven
//     purely by the main-process `onModelDownloadProgress` events;
//   * Cancel routes to `aiModel.cancelModelDownload`;
//   * installed packs surface their on-disk size usage.
//
// Bridge calls are recorded by the session-wide stub installed in
// `setup.vitest.ts`.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";

import { ModelManager } from "./ModelManager";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import type { ModelDownloadProgress } from "../../../shared/scene";

async function flushAsync(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

const SD15 = {
  id: "image_gen_sd15",
  name: "Stable Diffusion 1.5 (fp16)",
  kind: "sidecar",
  category: "generation",
  capabilities: ["image_generation"],
  sizeBytes: 2_132_696_762,
  sha256: "e9476a13728cd75d8279f6ec8bad753a66a1957ca375a1464dc63b37db6e3916",
  filePath: "stable-diffusion-v1-5.safetensors",
  installed: false,
  downloadUrl:
    "https://huggingface.co/Comfy-Org/stable-diffusion-v1-5-archive/resolve/main/v1-5-pruned-emaonly-fp16.safetensors",
};

/// Mid-tier limits with image generation explicitly allowed so the
/// generation pack passes the hard gate and renders.
function allowGeneration(stub: ReturnType<typeof kcreateStub>): void {
  stub.override("runtime.resourceLimits", () => ({
    deviceTier: "2",
    lowResourceMode: false,
    effectiveUndoDepth: 50,
    effectiveRasterCacheMb: 256,
    effectiveMaxModelMb: 8192,
    gpuRenderingAllowed: true,
    imageGenerationAllowed: true,
    visionModelMaxMb: 4096,
    platform: "Linux",
  }));
}

describe("ModelManager — download UX", () => {
  it("hides generation packs when image generation is not allowed", async () => {
    const stub = kcreateStub();
    // Default resourceLimits has imageGenerationAllowed: false.
    stub.override("aiModel.listModelPacks", () => [SD15]);

    render(<ModelManager onStatus={vi.fn()} />);
    await flushAsync();

    // Hard gate (not a soft one): the pack is filtered out entirely.
    expect(screen.queryByText(SD15.name)).toBeNull();
  });

  it("downloads a generation pack and renders a live progress bar", async () => {
    const stub = kcreateStub();
    allowGeneration(stub);
    stub.override("aiModel.listModelPacks", () => [SD15]);
    // Keep the download pending so the in-flight UI (progress bar +
    // Cancel) stays mounted for assertions.
    stub.override(
      "aiModel.downloadModelPack",
      () => new Promise<never>(() => undefined),
    );
    // Capture the progress listener the component registers so the
    // test can push main-process events at it.
    let emit: ((p: ModelDownloadProgress) => void) | null = null;
    stub.override("aiModel.onModelDownloadProgress", (...args: unknown[]) => {
      emit = args[0] as (p: ModelDownloadProgress) => void;
      return (): void => undefined;
    });

    render(<ModelManager onStatus={vi.fn()} />);
    await flushAsync();

    expect(screen.getByText(SD15.name)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^download$/i }));
    await flushAsync();

    const call = stub.calls.find((c) => c.method === "aiModel.downloadModelPack");
    expect(call).toBeDefined();
    expect(call?.args[0]).toBe("image_gen_sd15");

    // The progress bar mounts immediately (before the first event).
    const bar = screen.getByRole("progressbar");
    expect(bar).toBeTruthy();
    expect(bar.getAttribute("aria-busy")).toBe("true");

    // Drive a determinate downloading event: 1.0 GB / 2.1 GB ≈ 47%.
    expect(emit).not.toBeNull();
    await act(async () => {
      emit?.({
        packId: "image_gen_sd15",
        phase: "downloading",
        receivedBytes: 1_000_000_000,
        totalBytes: 2_132_696_762,
        message: "",
      });
    });

    const updated = screen.getByRole("progressbar");
    expect(updated.getAttribute("aria-valuenow")).toBe("47");
    expect(screen.getByText(/47% · 1\.0 GB \/ 2\.1 GB/)).toBeTruthy();
  });

  it("routes Cancel to the bridge while a download is in flight", async () => {
    const stub = kcreateStub();
    allowGeneration(stub);
    stub.override("aiModel.listModelPacks", () => [SD15]);
    stub.override(
      "aiModel.downloadModelPack",
      () => new Promise<never>(() => undefined),
    );

    render(<ModelManager onStatus={vi.fn()} />);
    await flushAsync();

    fireEvent.click(screen.getByRole("button", { name: /^download$/i }));
    await flushAsync();

    const cancel = screen.getByRole("button", { name: /^cancel$/i });
    fireEvent.click(cancel);
    await flushAsync();

    expect(
      stub.calls.some((c) => c.method === "aiModel.cancelModelDownload"),
    ).toBe(true);
  });

  it("summarizes installed-pack disk usage", async () => {
    const stub = kcreateStub();
    allowGeneration(stub);
    stub.override("aiModel.listModelPacks", () => [
      { ...SD15, installed: true },
    ]);

    render(<ModelManager onStatus={vi.fn()} />);
    await flushAsync();

    // 2_132_696_762 bytes → formatBytes → "2.1 GB".
    expect(screen.getByText(/1 installed · 2\.1 GB on disk/)).toBeTruthy();
  });
});
