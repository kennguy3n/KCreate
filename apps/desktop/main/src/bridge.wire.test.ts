// @vitest-environment node
//
// Wire-format lockstep guard (AGENTS.md Rule 4) for the
// `#[napi(object)]` bridge exports whose multi-word fields silently
// read back `undefined` when the preload mis-modelled them as
// snake_case — the bug behind the blank RAM badge (P2-7), the empty
// properties "Type" field (P2-8), and the frozen `ResponsivePreview`
// (`frame_id` → `frameId`).
//
// `renderer_frame_info`, `renderer_acquire_frame`, `document_get_tree`,
// `document_status`, `project_*` and `runtime_status` are exported from
// `crates/kcreate_bridge/src/lib.rs` as `#[napi(object)]` structs.
// napi-rs rewrites their snake_case Rust field identifiers to camelCase
// on the JS side (`node_type` → `nodeType`, `total_ram_mb` →
// `totalRamMb`, `frame_id` → `frameId`), so the raw bridge return types
// (`*Napi` / `*Snake` in `bridge.ts`) and the public DTOs in
// `shared/scene.ts` must agree on those camelCase names. That agreement
// is exactly what lets `preload.ts` cast the IPC results straight to
// the public types instead of running a (previously broken) converter.
//
// If any field regresses to snake_case, one of the assignments below
// stops compiling and `pnpm typecheck` fails before the bad casing can
// ship.
import { test, expect } from "vitest";
import type {
  FrameInfoNapi,
  AcquiredFrameNapi,
  ProjectInfoSnake,
  NodeInfoSnake,
  RuntimeStatusSnake,
  DocumentStatusSnake,
  ThumbnailBytesSnake,
  TemplateInstantiateResultSnake,
} from "./bridge";
import type {
  FrameInfo,
  AcquiredFrame,
  ProjectInfo,
  NodeInfo,
  RuntimeStatus,
  DocumentStatus,
  ThumbnailBytes,
  TemplateInstantiateReport,
} from "../../shared/scene";

/** Fails to compile unless `value`'s type is assignable to `T`. */
function expectAssignable<T>(_value: T): void {
  void _value;
}

// The raw bridge wire shapes must be assignable to the public DTOs the
// preload casts them to. `ProjectInfo` / `RuntimeStatus` /
// `DocumentStatus` mirror their napi structs field-for-field.
//
// `FrameInfo` / `AcquiredFrame` are guarded the same way: their napi
// structs emit `frameId` / `byteLength`, so a regression back to
// `frame_id` / `byte_length` (which froze `ResponsivePreview` after the
// first frame) stops compiling here.
expectAssignable<FrameInfo>({} as FrameInfoNapi);
expectAssignable<AcquiredFrame>({} as AcquiredFrameNapi);
expectAssignable<ProjectInfo>({} as ProjectInfoSnake);
expectAssignable<RuntimeStatus>({} as RuntimeStatusSnake);
expectAssignable<DocumentStatus>({} as DocumentStatusSnake);

// `NodeInfo` is guarded in BOTH directions so the lockstep can't be
// silently weakened (a `Pick<NodeInfo, keyof NodeInfoSnake>` guard would
// pass even if `NodeInfoSnake` dropped a required field like `version`).
//   1. The napi struct must be a valid public `NodeInfo` — catches a
//      dropped/renamed/snake_cased required field.
//   2. It must carry *nothing* the public DTO lacks. The only fields on
//      `NodeInfo` the napi struct never emits are the optional
//      `componentInstance` / `metadata`, so stripping those two yields
//      exactly `NodeInfoSnake` — catches a stray field on either side.
expectAssignable<NodeInfo>({} as NodeInfoSnake);
expectAssignable<NodeInfoSnake>(
  {} as Omit<NodeInfo, "componentInstance" | "metadata">,
);

// G2 template library: `templateThumbnail` reuses the `ThumbnailBytes`
// napi struct (so a `byte_size` → `byteSize` regression breaks here),
// and `templateInstantiate` returns `TemplateInstantiateResult`
// (`artboard_id` → `artboardId`, `node_ids` → `nodeIds`). Guarded in
// both directions so neither side can drift a field.
expectAssignable<ThumbnailBytes>({} as ThumbnailBytesSnake);
expectAssignable<ThumbnailBytesSnake>({} as ThumbnailBytes);
expectAssignable<TemplateInstantiateReport>(
  {} as TemplateInstantiateResultSnake,
);
expectAssignable<TemplateInstantiateResultSnake>(
  {} as TemplateInstantiateReport,
);

test("public runtime/document DTOs expose their multi-word fields in camelCase", () => {
  // Shaped exactly like the napi objects the bridge returns. These are
  // the multi-word keys the old snake_case converters dropped to
  // `undefined`.
  const runtime: RuntimeStatus = {
    deviceTier: "Tier2",
    gpuAvailable: true,
    gpuName: null,
    platform: "LinuxX64",
    totalRamMb: 32110,
  };
  const status: DocumentStatus = {
    nodeCount: 13,
    canUndo: true,
    canRedo: false,
    undoDepth: 11,
    redoDepth: 0,
  };

  expect(typeof runtime.totalRamMb).toBe("number");
  expect(runtime.deviceTier.length).toBeGreaterThan(0);
  expect(typeof status.nodeCount).toBe("number");
  expect(typeof status.canUndo).toBe("boolean");
});
