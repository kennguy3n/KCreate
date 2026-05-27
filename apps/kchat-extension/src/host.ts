// Typed host-procedure interface for the KCreate companion
// extension.
//
// The KChat Desktop host (see uneycom/uney-chat-desktop docs/
// proposals/foundation/01-architecture.md §6.16) exposes its
// procedure registry to extensions through a single global
// injected into the bundle's sandbox: `globalThis.__kchatHost`.
//
// The host's runtime is responsible for:
//   - Validating the call against the manifest's `procedures` list
//     (denying calls not declared)
//   - Running the consent gateway + capability gateway
//   - Validating the request payload (the host owns the schema)
//   - Returning a public-safe DTO (redactor applied)
//
// This module just gives us a typed wrapper that:
//   - Asserts the host bridge exists (extensions running outside the
//     host crash loudly instead of silently dropping calls)
//   - Validates the *response* shape with Zod so a host-side
//     contract drift surfaces as a typed error in the extension
//     rather than a `cannot read property of undefined` further
//     down the React tree.
import { z } from "zod";

const HostProcedureCallSchema = z.function();

const HostBridgeSchema = z.object({
  /**
   * Invoke a host procedure declared in the extension's manifest.
   * Resolves with the host's typed response or rejects with a
   * `HostProcedureError` describing the failure category.
   */
  invokeProcedure: HostProcedureCallSchema,
  /**
   * Open a deeplink. The host owns the deeplink allowlist
   * (`src/core/domain/deeplink/host-app-routes.ts`) — unknown URLs
   * collapse to `HOST_APP_ROUTE_DENIED`.
   */
  openDeeplink: HostProcedureCallSchema,
  /**
   * Subscribe to a host event stream. Returns an `unsubscribe`
   * function.
   */
  subscribe: HostProcedureCallSchema,
});

export type HostProcedureErrorKind =
  | "EXTENSION_CAPABILITY_DENIED"
  | "EXTENSION_NOT_INSTALLED"
  | "CONSENT_REQUIRED"
  | "RATE_LIMITED"
  | "INVALID_REQUEST"
  | "HOST_INTERNAL_ERROR"
  | "HOST_PROCEDURE_NOT_FOUND";

export class HostProcedureError extends Error {
  public readonly kind: HostProcedureErrorKind;
  public readonly procedureId: string;
  constructor(
    kind: HostProcedureErrorKind,
    procedureId: string,
    message: string,
  ) {
    super(`${kind} (${procedureId}): ${message}`);
    this.name = "HostProcedureError";
    this.kind = kind;
    this.procedureId = procedureId;
  }
}

interface RawHostBridge {
  invokeProcedure(
    id: string,
    payload: unknown,
  ): Promise<{
    ok: boolean;
    value?: unknown;
    error?: { kind: string; message: string };
  }>;
  openDeeplink(url: string): Promise<{ ok: boolean; message?: string }>;
  subscribe(
    topic: string,
    listener: (value: unknown) => void,
  ): () => void;
}

declare global {
  var __kchatHost: RawHostBridge | undefined;
}

// Holds the most recently validated bridge reference. We compare
// by identity so the (cheap) `Zod.parse` shape check only runs
// when the bridge object itself changes — typically once at host
// inject time, plus once per test seam swap. Avoids paying the
// validation cost on every host procedure invocation.
let validatedBridge: RawHostBridge | undefined;

function host(): RawHostBridge {
  const bridge = globalThis.__kchatHost;
  if (!bridge) {
    throw new HostProcedureError(
      "EXTENSION_NOT_INSTALLED",
      "(host)",
      "host bridge not injected — extension is running outside KChat Desktop",
    );
  }
  if (bridge !== validatedBridge) {
    // Run the shape check the first time we see this bridge
    // reference so a malformed bridge fails fast at the first call
    // site instead of producing confusing downstream errors.
    HostBridgeSchema.parse(bridge);
    validatedBridge = bridge;
  }
  return bridge;
}

const KNOWN_ERROR_KINDS: ReadonlySet<HostProcedureErrorKind> = new Set([
  "EXTENSION_CAPABILITY_DENIED",
  "EXTENSION_NOT_INSTALLED",
  "CONSENT_REQUIRED",
  "RATE_LIMITED",
  "INVALID_REQUEST",
  "HOST_INTERNAL_ERROR",
  "HOST_PROCEDURE_NOT_FOUND",
]);

function asKnownKind(kind: string): HostProcedureErrorKind {
  return (KNOWN_ERROR_KINDS as ReadonlySet<string>).has(kind)
    ? (kind as HostProcedureErrorKind)
    : "HOST_INTERNAL_ERROR";
}

export async function invokeProcedure<T>(
  procedureId: string,
  payload: unknown,
  responseSchema: z.ZodType<T>,
): Promise<T> {
  const raw = await host().invokeProcedure(procedureId, payload);
  if (!raw.ok) {
    const err = raw.error ?? {
      kind: "HOST_INTERNAL_ERROR",
      message: "no error body returned",
    };
    throw new HostProcedureError(asKnownKind(err.kind), procedureId, err.message);
  }
  return responseSchema.parse(raw.value);
}

export async function openDeeplink(url: string): Promise<void> {
  const result = await host().openDeeplink(url);
  if (!result.ok) {
    throw new HostProcedureError(
      "EXTENSION_CAPABILITY_DENIED",
      "deeplink.open_external",
      result.message ?? "host refused deeplink",
    );
  }
}

export function subscribe<T>(
  topic: string,
  schema: z.ZodType<T>,
  listener: (value: T) => void,
): () => void {
  return host().subscribe(topic, (raw) => {
    const parsed = schema.safeParse(raw);
    if (parsed.success) {
      listener(parsed.data);
    } else {
      console.warn(
        `[kcreate-companion] dropped malformed event on topic ${topic}:`,
        parsed.error.format(),
      );
    }
  });
}

/**
 * Test-only seam. Lets the unit-test harness inject a fake bridge
 * without needing the host runtime. Never called from production
 * code paths.
 */
export function __setHostBridgeForTests(bridge: RawHostBridge | undefined): void {
  // Drop the validated-bridge memo so the next `host()` call
  // re-validates the new fake bridge (or, after clearing, the next
  // production call re-validates the real bridge once it's
  // re-injected).
  validatedBridge = undefined;
  if (bridge === undefined) {
    globalThis.__kchatHost = undefined;
    return;
  }
  globalThis.__kchatHost = bridge;
}
