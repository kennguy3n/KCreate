// HomePage starter tests (Phase A4).
//
// Asserts the surface contract the rest of the app relies on:
//   * all 8 job-first create cards from `CREATE_OPTIONS` render;
//   * clicking a card calls `onOpenEditor` with the matching id;
//   * the "Get started with a brief" tile opens the brief modal.
//
// These tests deliberately avoid asserting layout (positions,
// colours, spacing) — those are visual concerns covered by the
// design tokens, not by component-level unit tests.

import { describe, it, expect } from "vitest";
import {
  render,
  screen,
  fireEvent,
  act,
  waitFor,
} from "@testing-library/react";

import { CREATE_OPTIONS, HomePage } from "./HomePage";
import type { Preferences } from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";

async function flushAsync() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function prefsWithOnboarding(completed: boolean): Preferences {
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
      completed,
      lastSeenPackId: null,
    },
  };
}

function escapeRegex(input: string): string {
  return input.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function renderHome() {
  let lastJobKind: string | null = null;
  const utils = render(
    <HomePage
      onOpenEditor={(jobKind) => {
        lastJobKind = jobKind;
      }}
      onOpenProject={() => {}}
      onBriefApplied={() => {}}
    />,
  );
  return {
    ...utils,
    captured: {
      get lastJobKind() {
        return lastJobKind;
      },
    },
  };
}

describe("HomePage", () => {
  it("renders all 8 job-first create options", () => {
    renderHome();
    expect(CREATE_OPTIONS.length).toBe(8);
    for (const opt of CREATE_OPTIONS) {
      expect(
        screen.getByRole("button", { name: new RegExp(escapeRegex(opt.title), "i") }),
        `card "${opt.title}" should be reachable by accessible name`,
      ).toBeInTheDocument();
    }
  });

  it("invokes onOpenEditor with the card's job id when clicked", () => {
    const { captured } = renderHome();
    const target = CREATE_OPTIONS[0]!;
    fireEvent.click(
      screen.getByRole("button", { name: new RegExp(escapeRegex(target.title), "i") }),
    );
    expect(captured.lastJobKind).toBe(target.id);
  });

  it("exposes the brief-modal opener as a button", () => {
    renderHome();
    // The brief tile is labelled "Start from a brief" in the JSX —
    // pinning it down by accessible name so a future copy change
    // surfaces here.
    expect(
      screen.getByRole("button", { name: /brief/i }),
    ).toBeInTheDocument();
  });

  // Phase C — welcome modal wiring. HomePage owns the
  // preferences-load lifecycle and the dismiss-time persist.
  it("auto-opens the WelcomeModal when preferences.onboarding.completed === false", async () => {
    const stub = kcreateStub();
    stub.override("phase10.preferencesLoad", () =>
      prefsWithOnboarding(false),
    );

    renderHome();
    await waitFor(() => {
      expect(
        screen.getByTestId("kcreate-welcome-modal"),
      ).toBeInTheDocument();
    });
  });

  it("keeps the WelcomeModal closed for returning users (completed === true)", async () => {
    const stub = kcreateStub();
    stub.override("phase10.preferencesLoad", () =>
      prefsWithOnboarding(true),
    );

    renderHome();
    await flushAsync();
    expect(screen.queryByTestId("kcreate-welcome-modal")).toBeNull();
  });

  it("persists onboarding.completed=true after dismiss", async () => {
    const stub = kcreateStub();
    stub.override("phase10.preferencesLoad", () =>
      prefsWithOnboarding(false),
    );
    let savedPrefs: Preferences | null = null;
    stub.override("phase10.preferencesSave", (...args: unknown[]) => {
      savedPrefs = args[0] as Preferences;
      return undefined;
    });

    renderHome();
    await waitFor(() => {
      expect(screen.getByTestId("kcreate-welcome-modal")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("kcreate-welcome-skip"));
    await flushAsync();

    expect(savedPrefs).not.toBeNull();
    expect(savedPrefs!.onboarding.completed).toBe(true);
    expect(screen.queryByTestId("kcreate-welcome-modal")).toBeNull();
  });
});
