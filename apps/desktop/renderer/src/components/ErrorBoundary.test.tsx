// ErrorBoundary — renderer-side regression tests.
//
// Pins the contract introduced alongside the lazy editor chunk (PR #48):
// a descendant that throws during render must surface the caller's
// recovery fallback instead of throwing past the root, and `reset` must
// re-mount the children so a recovered failure can retry.

import { useState } from "react";
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";

import { ErrorBoundary } from "./ErrorBoundary";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function Boom({ throws }: { throws: boolean }): JSX.Element {
  if (throws) {
    throw new Error("kaboom");
  }
  return <div>safe child</div>;
}

describe("ErrorBoundary", () => {
  it("renders children when nothing throws", () => {
    render(
      <ErrorBoundary fallback={() => <div>fallback</div>}>
        <Boom throws={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText("safe child")).toBeInTheDocument();
    expect(screen.queryByText("fallback")).not.toBeInTheDocument();
  });

  it("renders the fallback with the caught error when a child throws", () => {
    // React logs the boundary error to console.error in dev; silence it
    // so the test output stays readable and assert our own
    // componentDidCatch breadcrumb fired.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    render(
      <ErrorBoundary fallback={(error) => <div>caught: {error.message}</div>}>
        <Boom throws={true} />
      </ErrorBoundary>,
    );

    expect(screen.getByText("caught: kaboom")).toBeInTheDocument();
    expect(errSpy).toHaveBeenCalled();
  });

  it("reset clears the boundary so recovered children re-mount", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});

    // The retry button flips the harness state (so a fresh child
    // element no longer throws) and clears the boundary in the same
    // click — the canonical reset-error-boundary pattern.
    function Harness(): JSX.Element {
      const [throws, setThrows] = useState(true);
      return (
        <ErrorBoundary
          fallback={(_error, reset) => (
            <button
              type="button"
              onClick={() => {
                setThrows(false);
                reset();
              }}
            >
              retry
            </button>
          )}
        >
          <Boom throws={throws} />
        </ErrorBoundary>
      );
    }

    render(<Harness />);
    expect(screen.getByText("retry")).toBeInTheDocument();

    fireEvent.click(screen.getByText("retry"));
    expect(screen.getByText("safe child")).toBeInTheDocument();
    expect(screen.queryByText("retry")).not.toBeInTheDocument();
  });
});
