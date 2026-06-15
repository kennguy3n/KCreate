// @vitest-environment node
//
// Wire-format lockstep guard (AGENTS.md Rule 4) for the four
// `#[napi(object)]` bridge exports whose multi-word fields silently
// read back `undefined` when the preload mis-modelled them as
// snake_case — the bug behind the blank RAM badge (P2-7) and the empty
// properties "Type" field (P2-8).
//
// `document_get_tree`, `document_status`, `project_*` and
// `runtime_status` are exported from `crates/kcreate_bridge/src/lib.rs`
// as `#[napi(object)]` structs. napi-rs rewrites their snake_case Rust
// field identifiers to camelCase on the JS side (`node_type` →
// `nodeType`, `total_ram_mb` → `totalRamMb`), so the raw bridge return
// types (`*Snake` in `bridge.ts`) and the public DTOs in
// `shared/scene.ts` must agree on those camelCase names. That agreement
// is exactly what lets `preload.ts` cast the IPC results straight to
// the public types instead of running a (previously broken) converter.
//
// If any field regresses to snake_case, one of the assignments below
// stops compiling and `pnpm typecheck` fails before the bad casing can
// ship.
import { test, expect } from "vitest";
import type {
  ProjectInfoSnake,
  NodeInfoSnake,
  RuntimeStatusSnake,
  DocumentStatusSnake,
} from "./bridge";
import type {
  ProjectInfo,
  NodeInfo,
  RuntimeStatus,
  DocumentStatus,
} from "../../shared/scene";

/** Fails to compile unless `value`'s type is assignable to `T`. */
function expectAssignable<T>(_value: T): void {
  void _value;
}

// The raw bridge wire shapes must be assignable to the public DTOs the
// preload casts them to. `ProjectInfo` / `RuntimeStatus` /
// `DocumentStatus` are modelled in full; `NodeInfoSnake` deliberately
// carries only the subset of `NodeInfo` the layer tree needs (no
// `version` / `componentInstance` / `metadata`), so its guard is
// restricted to the fields it does declare.
expectAssignable<ProjectInfo>({} as ProjectInfoSnake);
expectAssignable<RuntimeStatus>({} as RuntimeStatusSnake);
expectAssignable<DocumentStatus>({} as DocumentStatusSnake);
expectAssignable<Pick<NodeInfo, keyof NodeInfoSnake>>({} as NodeInfoSnake);

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
