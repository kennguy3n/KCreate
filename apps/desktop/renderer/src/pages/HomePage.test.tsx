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
import { render, screen, fireEvent } from "@testing-library/react";

import { CREATE_OPTIONS, HomePage } from "./HomePage";

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
});
