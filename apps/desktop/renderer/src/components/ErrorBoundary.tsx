import { Component, type ErrorInfo, type ReactNode } from "react";

interface ErrorBoundaryProps {
  /**
   * Renders the recovery UI when a descendant throws during render or
   * commit. Receives the caught error plus a `reset` that clears the
   * boundary and re-mounts `children` — used to retry after a
   * recoverable failure.
   */
  fallback: (error: Error, reset: () => void) => ReactNode;
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/**
 * Render-phase error boundary. React exposes no hook equivalent for
 * `componentDidCatch`, so this is a class component by necessity.
 *
 * It exists so a failed lazy-chunk load — a corrupted asar, or the
 * editor chunk missing after a partial auto-update — surfaces a
 * recoverable error UI instead of throwing past the root and unmounting
 * the whole tree to a blank white screen. Kept generic: the caller
 * supplies the fallback so the recovery affordances stay with the
 * routing logic that owns them.
 */
export class ErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  override state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return {
      error: error instanceof Error ? error : new Error(String(error)),
    };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    // Breadcrumb for the devtools; the rendered fallback is the
    // user-facing recovery path.
    console.error(
      "kcreate: render boundary caught an error",
      error,
      info.componentStack,
    );
  }

  private readonly reset = (): void => {
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (error !== null) {
      return this.props.fallback(error, this.reset);
    }
    return this.props.children;
  }
}
