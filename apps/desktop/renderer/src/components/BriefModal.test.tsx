// BriefModal tests (G3 — Gamma-style themed multi-page generator).
//
// Pins down the "Start from a brief" modal surface:
//   * the modal renders nothing while closed;
//   * the themed-design flow is the default and works with NO local
//     model loaded (the deterministic `kcreate_ai::themed_deck`
//     generator), so the "Generate" button is reachable regardless of
//     `llmReady`;
//   * "Generate" forwards the typed brief plus the resolved
//     `ThemedDesignOptions` (format + theme + page-size + section
//     count + LLM opt-in) to `phase10.aiGenerateThemedDesign`, and the
//     bridge result is handed back through `onApplied`;
//   * the page-size control only contributes to the options when the
//     one-pager format is selected;
//   * `useLlm` is only forwarded when the sidecar is ready (the toggle
//     is `useLlm && llmReady`), so an enrichment request can never be
//     emitted against an unloaded model;
//   * the "Single artboard" plan mode stays gated on `llmReady`
//     because it has no deterministic fallback;
//   * a bridge rejection surfaces as an inline error and never fires
//     `onApplied`.
//
// Mounts under the session-wide kcreate stub installed in
// `setup.vitest.ts`. `document.getProjectInfo` is overridden to a
// non-null project so `ensureProject` is a no-op (the scratch-project
// materialisation path is exercised by the App-level integration, not
// here).

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, act, waitFor } from "@testing-library/react";

import { BriefModal } from "./BriefModal";
import type {
  BriefApplyResult,
  ProjectInfo,
  ThemedDesignApplyResult,
  ThemedDesignOptions,
} from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";

async function flushAsync() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

const FAKE_PROJECT: ProjectInfo = {
  id: "00000000-0000-0000-0000-000000000001",
  name: "scratch",
  path: "/tmp/scratch.kstudio",
  createdAt: "2026-01-01T00:00:00Z",
  modifiedAt: "2026-01-01T00:00:00Z",
};

function fakeResult(
  overrides: Partial<ThemedDesignApplyResult> = {},
): ThemedDesignApplyResult {
  return {
    pageId: "page-1",
    artboardIds: ["ab-1", "ab-2"],
    brandKitId: "brand-1",
    slideCount: 6,
    themeId: "midnight",
    themeName: "Midnight",
    format: "deck",
    usedLlm: false,
    ...overrides,
  };
}

interface MountOptions {
  llmReady?: boolean;
  open?: boolean;
}

function mount(opts: MountOptions = {}) {
  const stub = kcreateStub();
  // `ensureProject` short-circuits when a project is already open.
  stub.override("document.getProjectInfo", () => FAKE_PROJECT);
  const applied =
    vi.fn<(result: BriefApplyResult | ThemedDesignApplyResult) => void>();
  const closed = vi.fn<() => void>();
  const utils = render(
    <BriefModal
      open={opts.open ?? true}
      onClose={closed}
      llmReady={opts.llmReady ?? false}
      onApplied={applied}
    />,
  );
  return { ...utils, stub, applied, closed };
}

function lastGenerateCall(stub: ReturnType<typeof kcreateStub>) {
  const calls = stub.calls.filter(
    (c) => c.method === "phase10.aiGenerateThemedDesign",
  );
  return calls.at(-1);
}

describe("BriefModal — visibility", () => {
  it("renders nothing when closed", () => {
    const { container } = mount({ open: false });
    expect(container.firstChild).toBeNull();
  });

  it("defaults to the themed-design mode with controls visible", () => {
    mount();
    expect(screen.getByTestId("kcreate-themed-controls")).toBeInTheDocument();
    // Both format options + all five themes render.
    expect(screen.getByTestId("kcreate-themed-format-deck")).toBeInTheDocument();
    expect(
      screen.getByTestId("kcreate-themed-format-onepager"),
    ).toBeInTheDocument();
    for (const id of ["midnight", "sunrise", "forest", "ember", "slate"]) {
      expect(
        screen.getByTestId(`kcreate-themed-theme-${id}`),
        `theme chip ${id} should render`,
      ).toBeInTheDocument();
    }
  });
});

describe("BriefModal — themed generation", () => {
  it("disables Generate until a non-empty brief is typed", () => {
    mount();
    const generate = screen.getByTestId(
      "kcreate-themed-generate",
    ) as HTMLButtonElement;
    expect(generate.disabled).toBe(true);

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "   " },
    });
    expect(generate.disabled, "whitespace-only brief stays disabled").toBe(
      true,
    );

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "Pitch deck for an indie coffee roaster" },
    });
    expect(generate.disabled).toBe(false);
  });

  it("forwards the brief + default deck options and applies the result", async () => {
    const { stub, applied } = mount();
    const result = fakeResult();
    stub.override("phase10.aiGenerateThemedDesign", () => result);

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "Pitch deck for an indie coffee roaster" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-generate"));
    await flushAsync();

    const call = lastGenerateCall(stub);
    expect(call, "Generate should invoke the themed bridge").toBeDefined();
    expect(call?.args[0]).toBe("Pitch deck for an indie coffee roaster");
    const options = call?.args[1] as ThemedDesignOptions;
    expect(options).toEqual({
      format: "deck",
      themeId: "midnight",
      useLlm: false,
    });
    // A deck request must not leak the one-pager-only page-size field.
    expect(options.onePagerSize).toBeUndefined();
    expect(options.sectionCount).toBeUndefined();

    expect(applied).toHaveBeenCalledTimes(1);
    expect(applied).toHaveBeenCalledWith(result);
  });

  it("captures the selected theme and explicit slide count", async () => {
    const { stub } = mount();
    stub.override("phase10.aiGenerateThemedDesign", () =>
      fakeResult({ themeId: "forest", themeName: "Forest", slideCount: 8 }),
    );

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "Quarterly results for a forestry co-op" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-theme-forest"));
    fireEvent.change(screen.getByTestId("kcreate-themed-sections"), {
      target: { value: "8" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-generate"));
    await flushAsync();

    const options = lastGenerateCall(stub)?.args[1] as ThemedDesignOptions;
    expect(options.themeId).toBe("forest");
    expect(options.sectionCount).toBe(8);
    expect(options.format).toBe("deck");
  });

  it("includes the page size only for the one-pager format", async () => {
    const { stub } = mount();
    stub.override("phase10.aiGenerateThemedDesign", () =>
      fakeResult({ format: "onePager", slideCount: 1 }),
    );

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "One-page menu for a coffee bar" },
    });
    // The page-size control is hidden until one-pager is chosen.
    expect(screen.queryByTestId("kcreate-themed-size")).toBeNull();

    fireEvent.click(screen.getByTestId("kcreate-themed-format-onepager"));
    fireEvent.change(screen.getByTestId("kcreate-themed-size"), {
      target: { value: "letter" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-generate"));
    await flushAsync();

    const options = lastGenerateCall(stub)?.args[1] as ThemedDesignOptions;
    expect(options.format).toBe("onePager");
    expect(options.onePagerSize).toBe("letter");
  });
});

describe("BriefModal — LLM gating", () => {
  it("never forwards useLlm when the sidecar is not ready", async () => {
    const { stub } = mount({ llmReady: false });
    stub.override("phase10.aiGenerateThemedDesign", () => fakeResult());

    // The enrichment checkbox is disabled while the model is absent.
    const toggle = screen.getByTestId(
      "kcreate-themed-usellm",
    ) as HTMLInputElement;
    expect(toggle.disabled).toBe(true);

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "Deck about offline-first design" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-generate"));
    await flushAsync();

    const options = lastGenerateCall(stub)?.args[1] as ThemedDesignOptions;
    expect(options.useLlm).toBe(false);
  });

  it("forwards useLlm when the sidecar is ready and the toggle is on", async () => {
    const { stub } = mount({ llmReady: true });
    stub.override("phase10.aiGenerateThemedDesign", () =>
      fakeResult({ usedLlm: true }),
    );

    const toggle = screen.getByTestId(
      "kcreate-themed-usellm",
    ) as HTMLInputElement;
    expect(toggle.disabled).toBe(false);
    fireEvent.click(toggle);

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "Deck with AI-expanded talking points" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-generate"));
    await flushAsync();

    const options = lastGenerateCall(stub)?.args[1] as ThemedDesignOptions;
    expect(options.useLlm).toBe(true);
  });

  it("gates the Single-artboard plan mode on llmReady", () => {
    const { rerender } = mount({ llmReady: false });
    expect(
      (screen.getByTestId("kcreate-brief-mode-plan") as HTMLButtonElement)
        .disabled,
      "plan mode is disabled without a model",
    ).toBe(true);

    rerender(
      <BriefModal open onClose={() => {}} llmReady onApplied={() => {}} />,
    );
    expect(
      (screen.getByTestId("kcreate-brief-mode-plan") as HTMLButtonElement)
        .disabled,
      "plan mode is enabled once the model is ready",
    ).toBe(false);
  });
});

describe("BriefModal — error handling", () => {
  it("surfaces a bridge rejection and does not fire onApplied", async () => {
    const { stub, applied } = mount();
    stub.override("phase10.aiGenerateThemedDesign", () => {
      throw new Error("generator failed: empty outline");
    });

    fireEvent.change(screen.getByTestId("kcreate-brief-textarea"), {
      target: { value: "Deck that will fail" },
    });
    fireEvent.click(screen.getByTestId("kcreate-themed-generate"));
    await flushAsync();

    await waitFor(() => {
      expect(screen.getByTestId("kcreate-brief-error")).toBeInTheDocument();
    });
    expect(screen.getByTestId("kcreate-brief-error").textContent).toContain(
      "generator failed: empty outline",
    );
    expect(applied).not.toHaveBeenCalled();
  });
});
