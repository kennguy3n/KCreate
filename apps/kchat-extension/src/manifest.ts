// Extension manifest schema + builder.
//
// The KChat Desktop host (see uneycom/uney-chat-desktop docs/proposals/
// foundation/01-architecture.md §6.16 "Procedures registry" and
// §13.10 "A parallel descriptor catalogue") fixes a small contract
// the extension must declare statically:
//
//   - identity (id, version, display name, publisher key fingerprint)
//   - capabilities consumed (`procedures`) — namespaced host or
//     extension procedures the extension promises to call
//   - views contributed to slots — each view declares a slot id, a
//     surface-target compatibility, an entry file, and a title
//   - declared deeplinks the extension wants the host to route to it
//   - audit category opt-ins (used by the host's procedures audit
//     emitter to tag rows)
//
// The host validates this manifest with Zod at install time. We
// re-declare the schema here so the build script can also reject
// malformed manifests *before* signing — a corrupt manifest must
// never reach the host.
//
// `EXTENSION_SLOT_IDS` and `EXTENSION_SURFACE_TARGET_IDS` referenced
// here mirror the constants documented in
// `src/core/ui/components/extensions` (currently stubbed in main and
// living on a feature branch; the names are stable per the proposal).
import { z } from "zod";

export const ExtensionIdentitySchema = z.object({
  // `<publisher>.<extensionName>` per the proposal's
  // `extension.<extensionId>.<command>` procedure namespace rule.
  id: z
    .string()
    .regex(/^[a-z][a-z0-9-]*\.[a-z][a-z0-9-]*$/u, {
      message:
        "extension id must be `<publisher>.<extension-name>` in lower-kebab",
    }),
  version: z.string().regex(/^\d+\.\d+\.\d+(?:-[a-z0-9.-]+)?$/u, {
    message: "version must be SemVer 2.0.0",
  }),
  displayName: z.string().min(1).max(64),
  publisher: z.string().min(1).max(64),
  // Base64-URL-no-pad encoded Ed25519 verifying key — the signing
  // identity that the .kcz bundle was signed with. The host pins this
  // at install time so subsequent upgrades must be signed by the same
  // key (or an explicit re-trust flow runs).
  publisherPublicKey: z.string().regex(/^[A-Za-z0-9_-]{43}$/u, {
    message: "publisherPublicKey must be 43-char base64-url-no-pad Ed25519",
  }),
});

export const ExtensionProcedureSchema = z.object({
  // Either a host procedure (`<host-port>.<verb_snake_case>`) or an
  // extension procedure (`extension.<extensionId>.<command>`).
  id: z.string().regex(/^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$/u),
  // Audit category drives consent tier + audit tag + rate-limit
  // defaults. Must match one of the four categories documented in
  // §6.16.
  category: z.enum(["read", "context", "write", "upload"]),
  // Human description shown in the consent prompt.
  description: z.string().min(1).max(256),
});

export const ExtensionViewSchema = z.object({
  // The slot the view contributes to. The KChat Desktop host
  // enumerates these in `EXTENSION_SLOT_IDS`. We declare the ones
  // this extension uses verbatim (no wildcards) so a host upgrade
  // that renames a slot breaks loudly at install time.
  slot: z.enum([
    "outer-rightbar.community-context",
    "settings.developer.panel",
  ]),
  // Stable id for the view inside this extension; used by the host
  // when the user re-opens a previously-pinned surface.
  id: z.string().regex(/^[a-z][a-z0-9-]*$/u),
  title: z.string().min(1).max(64),
  // The JS entry point inside the .kcz bundle that renders this
  // view. Resolves relative to the bundle root.
  entry: z.string().regex(/^[A-Za-z0-9_./-]+\.js$/u),
  // Which surface targets the host may project this view onto. The
  // host validates compatibility at openSurface() time.
  surfaceTargets: z
    .array(z.enum(["outer-rightbar", "overlay-root"]))
    .min(1),
});

export const ExtensionDeeplinkSchema = z.object({
  // The deeplink scheme this extension handles inside the host.
  // The host's host-app-routes allowlist (§5.6 of 01-architecture.md)
  // routes matching prefixes to this extension's `onDeeplink`
  // handler.
  scheme: z.string().regex(/^[a-z][a-z0-9+-]*$/u),
  // The host-app route prefix that maps to this extension. Closed
  // form: `<scheme>://<host>` only — no wildcards.
  hostPrefix: z.string().min(1).max(64),
});

export const ExtensionManifestSchema = z.object({
  manifestVersion: z.literal(1),
  identity: ExtensionIdentitySchema,
  // Procedures the extension declares it will consume. The host
  // gates each call against this list at runtime — a procedure not
  // declared here returns `EXTENSION_CAPABILITY_DENIED` even if the
  // user previously granted consent.
  procedures: z.array(ExtensionProcedureSchema).min(1),
  // Views the extension contributes. At least one is required —
  // the proposal forbids "headless" extensions for the .kcz
  // surface (those go through MCP instead).
  contributes: z.object({
    views: z.array(ExtensionViewSchema).min(1),
    deeplinks: z.array(ExtensionDeeplinkSchema).default([]),
  }),
  // Optional rate limit overrides (per-minute budgets). Defaults
  // come from the procedure category.
  rateLimits: z
    .record(z.string(), z.number().int().positive())
    .optional(),
});

export type ExtensionManifest = z.infer<typeof ExtensionManifestSchema>;
export type ExtensionView = z.infer<typeof ExtensionViewSchema>;

/**
 * Parse + validate a manifest JSON value. Throws a
 * `z.ZodError` with the full path/message stack on invalid input.
 */
export function parseManifest(input: unknown): ExtensionManifest {
  return ExtensionManifestSchema.parse(input);
}
