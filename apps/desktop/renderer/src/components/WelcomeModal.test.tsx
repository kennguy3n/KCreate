// Phase C — WelcomeModal renderer-side regression tests.
//
// Covers the lifecycle states the modal can be in and pins the
// contract with the bridge:
//   1. mount with no recommendation → load phase → modal shows
//      the recommended pack name, tier badge, and three CTAs.
//   2. one-click install happy path → installRecommendedPack is
//      called, progress events update the bar, `onDismiss` fires
//      with the installed pack id once the user clicks "Get
//      started".
//   3. "I have the file" → reuses `pickModelFile` + `installModelPack`
//      and fires `onDismiss` with the pack id.
//   4. "Skip for now" → onDismiss(null), no install IPC fired.
//   5. install error → error message rendered, `onDismiss` fires
//      with null when the user closes.
//   6. unmount mid-install → `cancelInstall` IPC fired so the
//      download is aborted in the main process.

import { describe, it, expect } from "vitest";
import {
  render,
  screen,
  fireEvent,
  act,
  cleanup,
  waitFor,
} from "@testing-library/react";

import { LocaleProvider } from "../i18n";
import {
  WelcomeModal,
  shouldShowWelcomeModal,
} from "./WelcomeModal";
import type {
  ModelPack,
  OnboardingProgress,
  Preferences,
} from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";

const SAMPLE_PACK: ModelPack = {
  id: "llm_bonsai_1_7b",
  name: "Ternary-Bonsai 1.7B (Q2_0 GGUF)",
  kind: "sidecar",
  category: "core",
  capabilities: ["chat"],
  sizeBytes: 750_000_000,
  sha256: "",
  filePath: "",
  installed: false,
  downloadUrl: "https://huggingface.co/example/llm_bonsai_1_7b.gguf",
};

function makePrefs(overrides?: Partial<Preferences["onboarding"]>): Preferences {
  return {
    general: {
      theme: "system",
      language: "en-US",
      autosaveIntervalSec: 60,
      scratchProjectCleanupDays: 7,
    },
    canvas: {
      defaultGridSpacing: 8,
      defaultGridSubdivisions: 4,
      snapThresholdPx: 6,
      rulerUnits: "px",
    },
    ai: {
      defaultLlmModel: "",
      autoStartSidecar: false,
      gbnfGrammarDebugging: false,
    },
    performance: {
      rasterCacheBudgetMb: 256,
      undoDepthOverride: null,
      lowResourceMode: false,
    },
    privacy: {
      telemetryOptIn: false,
      auditLogRetentionDays: 30,
    },
    export: {
      lastDirByFormat: {},
      lastBatchDir: null,
    },
    onboarding: {
      completed: false,
      lastSeenPackId: null,
      ...overrides,
    },
  };
}

async function flushAsync() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("shouldShowWelcomeModal", () => {
  it("returns false when prefs are null (load not finished yet)", () => {
    expect(shouldShowWelcomeModal(null)).toBe(false);
  });

  it("returns true when onboarding.completed === false", () => {
    expect(shouldShowWelcomeModal(makePrefs({ completed: false }))).toBe(true);
  });

  it("returns false when onboarding.completed === true", () => {
    expect(shouldShowWelcomeModal(makePrefs({ completed: true }))).toBe(false);
  });
});

describe("WelcomeModal", () => {
  it("renders nothing when open=false", () => {
    const { container } = render(
      <WelcomeModal open={false} onDismiss={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("loads the recommended pack and renders name + tier + CTAs", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override("runtime.resourceLimits", () => ({
      deviceTier: "1",
      lowResourceMode: false,
      effectiveUndoDepth: 50,
      effectiveRasterCacheMb: 256,
      effectiveMaxModelMb: 4096,
      gpuRenderingAllowed: true,
      imageGenerationAllowed: false,
      visionModelMaxMb: 256,
      platform: "Linux",
    }));

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();

    expect(
      screen.getByTestId("kcreate-welcome-pack-name").textContent,
    ).toContain("Ternary-Bonsai 1.7B");
    expect(screen.getByText(/Tier 1/i)).toBeInTheDocument();
    expect(screen.getByTestId("kcreate-welcome-install")).toBeInTheDocument();
    expect(screen.getByTestId("kcreate-welcome-pick-file")).toBeInTheDocument();
    expect(screen.getByTestId("kcreate-welcome-skip")).toBeInTheDocument();
  });

  it("renders an error when the registry has no recommendation", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => "");
    stub.override("aiModel.listModelPacks", () => []);

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();

    expect(screen.getByTestId("kcreate-welcome-error").textContent).toMatch(
      /does not have a recommended local LLM pack/,
    );
  });

  it("renders an error when the recommended pack id is not in the registry", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => "llm_does_not_exist");
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();

    expect(screen.getByTestId("kcreate-welcome-error").textContent).toMatch(
      /not in the model registry/,
    );
  });

  it("clicking Skip after load passes the resolved pack id (for lastSeenPackId) and fires no install IPC", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    let dismissed: { called: boolean; id: string | null } = {
      called: false,
      id: null,
    };

    render(
      <WelcomeModal
        open={true}
        onDismiss={(id) => {
          dismissed = { called: true, id };
        }}
      />,
    );
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-skip"));

    expect(dismissed.called).toBe(true);
    // Skipping AFTER the recommendation has resolved still passes
    // the pack id back to the parent so HomePage can record it as
    // `lastSeenPackId` — this lets a future tier-rollover detector
    // notice when the recommended pack changes vs. what the user
    // last declined.
    expect(dismissed.id).toBe(SAMPLE_PACK.id);
    expect(
      stub.calls.some(
        (c) => c.method === "onboarding.installRecommendedPack",
      ),
    ).toBe(false);
  });

  it("clicking Skip BEFORE pack resolution passes null (load not finished)", async () => {
    const stub = kcreateStub();
    // Hold the recommendedPack call open forever so the modal
    // stays in the `loading` phase.
    stub.override(
      "llm.recommendedPack",
      () => new Promise(() => undefined),
    );
    let dismissed: { called: boolean; id: string | null } = {
      called: false,
      id: null,
    };

    render(
      <WelcomeModal
        open={true}
        onDismiss={(id) => {
          dismissed = { called: true, id };
        }}
      />,
    );
    // No flushAsync — we want to stay in the loading state. But
    // the close button does need to be in the DOM (it's rendered
    // outside the body switch); skipping inside the body is fine.
    fireEvent.click(screen.getByTestId("kcreate-welcome-close"));

    expect(dismissed.called).toBe(true);
    expect(dismissed.id).toBe(null);
  });

  it("clicking Install triggers installRecommendedPack and emits onDismiss with the pack id on Get started", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override("onboarding.installRecommendedPack", () => ({
      packId: SAMPLE_PACK.id,
      verified: true,
      actualSha256: "abc".repeat(20).slice(0, 64),
      sizeBytes: SAMPLE_PACK.sizeBytes,
    }));
    let dismissed: { called: boolean; id: string | null } = {
      called: false,
      id: null,
    };

    render(
      <WelcomeModal
        open={true}
        onDismiss={(id) => {
          dismissed = { called: true, id };
        }}
      />,
    );
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-install"));
    await flushAsync();

    expect(
      stub.calls.some(
        (c) => c.method === "onboarding.installRecommendedPack",
      ),
    ).toBe(true);

    await waitFor(() => {
      expect(screen.getByTestId("kcreate-welcome-done")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("kcreate-welcome-finish"));

    expect(dismissed.called).toBe(true);
    expect(dismissed.id).toBe(SAMPLE_PACK.id);
  });

  it("localizes the unverified-install summary (no pinned SHA-256 path)", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    // verified:false exercises the branch that used to be hard-coded
    // English regardless of locale (the reviewer-flagged i18n gap).
    stub.override("onboarding.installRecommendedPack", () => ({
      packId: SAMPLE_PACK.id,
      verified: false,
      actualSha256: "deadbeef".repeat(8),
      sizeBytes: SAMPLE_PACK.sizeBytes,
    }));

    render(
      <LocaleProvider initialLocale="es">
        <WelcomeModal open={true} onDismiss={() => {}} />
      </LocaleProvider>,
    );
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-install"));
    await flushAsync();

    const done = await screen.findByTestId("kcreate-welcome-done");
    // Spanish catalog copy, not the old hard-coded English string.
    expect(done.textContent).toContain(
      "sin SHA-256 fijado en el registro",
    );
    expect(done.textContent).not.toContain("no pinned SHA-256");
    // The actual-hash prefix is still interpolated into the summary.
    expect(done.textContent).toContain("deadbeefdead");
  });

  it("renders progress updates emitted by the main process", async () => {
    let emit: ((p: OnboardingProgress) => void) | null = null;
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override("onboarding.onInstallProgress", (cb: unknown) => {
      emit = cb as (p: OnboardingProgress) => void;
      return (): void => {
        emit = null;
      };
    });
    // Hold the install promise open so we can drive progress in
    // the meantime.
    let resolveInstall: ((value: unknown) => void) | null = null;
    stub.override(
      "onboarding.installRecommendedPack",
      () =>
        new Promise((resolve) => {
          resolveInstall = resolve;
        }),
    );

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-install"));
    await flushAsync();

    expect(emit).not.toBeNull();
    await act(async () => {
      emit?.({
        packId: SAMPLE_PACK.id,
        phase: "downloading",
        receivedBytes: 250_000_000,
        totalBytes: 750_000_000,
        message: "",
      });
    });

    const progressEl = screen.getByTestId("kcreate-welcome-progress");
    expect(progressEl.textContent).toMatch(/Downloading/);
    expect(progressEl.textContent).toMatch(/33%/);

    // Resolve to clean up the in-flight promise.
    await act(async () => {
      resolveInstall?.({
        packId: SAMPLE_PACK.id,
        verified: true,
        actualSha256: "0".repeat(64),
        sizeBytes: SAMPLE_PACK.sizeBytes,
      });
      await Promise.resolve();
    });
  });

  it("calls cancelInstall IPC when the modal closes mid-install", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    // Hold install open forever so the in-flight state persists.
    stub.override(
      "onboarding.installRecommendedPack",
      () => new Promise(() => undefined),
    );

    const { rerender } = render(
      <WelcomeModal open={true} onDismiss={() => {}} />,
    );
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-install"));
    await flushAsync();

    rerender(<WelcomeModal open={false} onDismiss={() => {}} />);
    await flushAsync();

    expect(
      stub.calls.filter((c) => c.method === "onboarding.cancelInstall").length,
    ).toBeGreaterThan(0);

    cleanup();
  });

  it("manual 'I have the file' path reuses pickModelFile + installModelPack", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override(
      "aiModel.pickModelFile",
      () => "/Users/test/Downloads/bonsai.gguf",
    );
    stub.override("aiModel.installModelPack", () => ({
      packId: SAMPLE_PACK.id,
      verified: true,
      actualSha256: "f".repeat(64),
      sizeBytes: SAMPLE_PACK.sizeBytes,
    }));
    let dismissed: { called: boolean; id: string | null } = {
      called: false,
      id: null,
    };

    render(
      <WelcomeModal
        open={true}
        onDismiss={(id) => {
          dismissed = { called: true, id };
        }}
      />,
    );
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-pick-file"));
    await flushAsync();

    expect(
      stub.calls.some((c) => c.method === "aiModel.pickModelFile"),
    ).toBe(true);
    expect(
      stub.calls.find((c) => c.method === "aiModel.installModelPack")?.args[0],
    ).toBe(SAMPLE_PACK.id);

    await waitFor(() => {
      expect(screen.getByTestId("kcreate-welcome-done")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("kcreate-welcome-finish"));
    expect(dismissed.id).toBe(SAMPLE_PACK.id);
  });

  it("install error renders the message and Close dismisses with null", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override("onboarding.installRecommendedPack", () => {
      throw new Error("network unreachable");
    });
    let dismissed: { called: boolean; id: string | null } = {
      called: false,
      id: null,
    };

    render(
      <WelcomeModal
        open={true}
        onDismiss={(id) => {
          dismissed = { called: true, id };
        }}
      />,
    );
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-install"));
    await flushAsync();

    await waitFor(() => {
      expect(screen.getByTestId("kcreate-welcome-error").textContent).toMatch(
        /network unreachable/,
      );
    });
    fireEvent.click(screen.getByTestId("kcreate-welcome-error-dismiss"));
    expect(dismissed.called).toBe(true);
    // The pack was resolved before the install error, so we still
    // pass its id through so HomePage can record `lastSeenPackId`.
    expect(dismissed.id).toBe(SAMPLE_PACK.id);
  });

  it("guards against double-click on the install button (single in-flight IPC)", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override(
      "onboarding.installRecommendedPack",
      () => new Promise(() => undefined),
    );

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();
    const btn = screen.getByTestId("kcreate-welcome-install");
    fireEvent.click(btn);
    // The button vanishes (replaced by Cancel) once installing
    // phase starts; the synchronous guard is the load-bearing
    // protection against a same-task double-click.
    await flushAsync();
    expect(
      stub.calls.filter(
        (c) => c.method === "onboarding.installRecommendedPack",
      ).length,
    ).toBe(1);
  });

  // Pins BUG_0001 from the round-2 Devin Review sweep on 70ccc3e.
  // Without the `prev.kind !== "installing"` guard in the install
  // catch updater, a Cancel-during-install would race the
  // cancelled-promise rejection: cancel moves to "loaded", catch
  // then unconditionally overrides to "error" with null tier/pack,
  // the PackCard disappears, and clicking Close persists
  // `onboarding.completed = true` (modal unreachable for cancel).
  it("Cancel during install returns to the loaded phase, not an error", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    // The install IPC rejects with the same "cancelled" sentinel
    // the main-process onboardingDownloader throws when the
    // cancel side-channel fires. This is the exact race the bug
    // describes: cancel handler runs → setPhase queues "loaded";
    // promise rejects → catch updater previously queued "error".
    let rejectInstall: (e: Error) => void = () => undefined;
    stub.override(
      "onboarding.installRecommendedPack",
      () =>
        new Promise((_resolve, reject) => {
          rejectInstall = reject;
        }),
    );

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();
    fireEvent.click(screen.getByTestId("kcreate-welcome-install"));
    await flushAsync();
    // Mid-install. Click Cancel — this fires cancelInstall IPC
    // and synchronously queues setPhase → "loaded".
    fireEvent.click(screen.getByTestId("kcreate-welcome-cancel"));
    // Now reject the in-flight install promise to simulate the
    // race: cancel updater already queued, rejection arrives next.
    rejectInstall(new Error("cancelled"));
    await flushAsync();

    // Pack card should still be visible (loaded phase preserved
    // tier/pack), error UI should NOT be present.
    expect(
      screen.queryByTestId("kcreate-welcome-error"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByTestId("kcreate-welcome-pack-name").textContent,
    ).toContain("Ternary-Bonsai 1.7B");
    expect(screen.getByTestId("kcreate-welcome-install")).toBeInTheDocument();
  });

  // Pins ANALYSIS_0001 from the round-2 Devin Review sweep on
  // 70ccc3e. The previous handleInstall set installInFlight.current
  // inside the setPhase updater (i.e. during React's commit phase,
  // AFTER setPhase returns). A same-task double-click between the
  // ref check and the React commit could pass the guard and fire
  // a second installRecommendedPack. The fix mirrors handlePickFile:
  // set the ref synchronously BEFORE any await.
  it("install ref is set synchronously before the IPC, so a same-task double-click never fires two IPCs", async () => {
    const stub = kcreateStub();
    stub.override("llm.recommendedPack", () => SAMPLE_PACK.id);
    stub.override("aiModel.listModelPacks", () => [SAMPLE_PACK]);
    stub.override(
      "onboarding.installRecommendedPack",
      () => new Promise(() => undefined),
    );

    render(<WelcomeModal open={true} onDismiss={() => {}} />);
    await flushAsync();
    const btn = screen.getByTestId("kcreate-welcome-install");
    // Two synchronous clicks in the same microtask, BEFORE
    // flushing. The synchronous ref guard must reject the second
    // click without waiting for React's commit phase.
    fireEvent.click(btn);
    fireEvent.click(btn);
    await flushAsync();
    expect(
      stub.calls.filter(
        (c) => c.method === "onboarding.installRecommendedPack",
      ).length,
    ).toBe(1);
  });
});
