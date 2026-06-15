// @vitest-environment node
//
// Regression guard for the systemic IPC failure-path bug: napi-rs
// synchronous exports built with `napi/dyn-symbols` return an `Error` as a
// *value* on the failure path instead of throwing it. Forwarded verbatim
// through `ipcMain.handle`, that makes the renderer's `invoke()` *resolve*
// with an Error object — so every preload caller that `JSON.parse`s the
// result crashes with "Unexpected token 'E', \"Error: …\" is not valid
// JSON" (e.g. `aiModel.detectTextRegions` / `extractPalette`) and callers
// that return the string verbatim hand back a bogus Error-shaped value
// (e.g. `aiModel.smartSelect`).
//
// `normalizeBridgeErrors` wraps the loaded bridge so a method that returns
// an `Error` throws it, restoring normal promise-rejection semantics across
// the whole IPC surface at a single chokepoint. These tests pin that
// contract without needing the native addon or an Electron host.
import { test, expect, vi } from "vitest";
import { normalizeBridgeErrors } from "./bridge";
import type { Bridge } from "./bridge";

// A minimal fake bridge exercising every shape a real method can take.
function fakeBridge(overrides: Record<string, unknown>): Bridge {
  return overrides as unknown as Bridge;
}

test("sync method returning an Error throws it instead of returning it", () => {
  const err = new Error("kcreate_bridge: node not found: 0000");
  const bridge = normalizeBridgeErrors(
    fakeBridge({ aiDetectTextRegions: () => err }),
  ) as unknown as { aiDetectTextRegions: () => unknown };
  expect(() => bridge.aiDetectTextRegions()).toThrowError(
    "kcreate_bridge: node not found: 0000",
  );
});

test("a thrown Error (the well-behaved path) still propagates", () => {
  const bridge = normalizeBridgeErrors(
    fakeBridge({
      aiSmartSelect: () => {
        throw new Error("boom");
      },
    }),
  ) as unknown as { aiSmartSelect: () => unknown };
  expect(() => bridge.aiSmartSelect()).toThrowError("boom");
});

test("sync method returning a normal value passes through unchanged", () => {
  const bridge = normalizeBridgeErrors(
    fakeBridge({ rendererFrameInfo: () => '{"frameId":7}' }),
  ) as unknown as { rendererFrameInfo: () => unknown };
  expect(bridge.rendererFrameInfo()).toBe('{"frameId":7}');
});

test("sync method returning an object (e.g. #[napi(object)]) passes through", () => {
  const obj = { frameId: 7, width: 1024 };
  const bridge = normalizeBridgeErrors(
    fakeBridge({ rendererAcquireFrame: () => obj }),
  ) as unknown as { rendererAcquireFrame: () => unknown };
  expect(bridge.rendererAcquireFrame()).toBe(obj);
});

test("sync method returning null/undefined passes through", () => {
  const bridge = normalizeBridgeErrors(
    fakeBridge({
      rendererAcquireFrame: () => null,
      rendererFrameInfo: () => undefined,
    }),
  ) as unknown as {
    rendererAcquireFrame: () => unknown;
    rendererFrameInfo: () => unknown;
  };
  expect(bridge.rendererAcquireFrame()).toBeNull();
  expect(bridge.rendererFrameInfo()).toBeUndefined();
});

test("async method resolving with a value is left untouched", async () => {
  const bridge = normalizeBridgeErrors(
    fakeBridge({ aiSuggestLayerNames: () => Promise.resolve('{"ok":true}') }),
  ) as unknown as { aiSuggestLayerNames: () => Promise<unknown> };
  await expect(bridge.aiSuggestLayerNames()).resolves.toBe('{"ok":true}');
});

test("async method rejecting still rejects (AsyncTask error path)", async () => {
  const bridge = normalizeBridgeErrors(
    fakeBridge({
      aiCheckAccessibility: () => Promise.reject(new Error("sidecar is not ready")),
    }),
  ) as unknown as { aiCheckAccessibility: () => Promise<unknown> };
  await expect(bridge.aiCheckAccessibility()).rejects.toThrowError(
    "sidecar is not ready",
  );
});

test("non-function properties pass through untouched", () => {
  const sentinel = { not: "a function" };
  const bridge = normalizeBridgeErrors(
    fakeBridge({ someData: sentinel }),
  ) as unknown as { someData: unknown };
  expect(bridge.someData).toBe(sentinel);
});

test("the same method reference is returned across reads (stable identity)", () => {
  const bridge = normalizeBridgeErrors(
    fakeBridge({ rendererFrameInfo: () => "x" }),
  ) as unknown as { rendererFrameInfo: () => unknown };
  expect(bridge.rendererFrameInfo).toBe(bridge.rendererFrameInfo);
});

test("the original method runs with the bridge as receiver", () => {
  const spy = vi.fn(function (this: unknown) {
    return this;
  });
  const raw = fakeBridge({ rendererFrameInfo: spy });
  const bridge = normalizeBridgeErrors(raw) as unknown as {
    rendererFrameInfo: () => unknown;
  };
  expect(bridge.rendererFrameInfo()).toBe(raw);
});
