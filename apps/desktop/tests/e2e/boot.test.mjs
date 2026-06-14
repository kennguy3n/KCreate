// End-to-end smoke test: boots the packaged Electron shell with the real
// native bridge and verifies the regressions that survived twelve "complete"
// phases because nothing in CI ever booted the app:
//
//   * P0-1 — the app boots from a clean `pnpm build` to the HomePage.
//   * P0-2 — a document mutation actually reaches the HTML `<canvas>`: the
//     Rust renderer's frame is presented and the canvas repaints. This is the
//     full document -> scene -> invalidate -> render -> readback -> putImageData
//     loop, end to end, in a real Electron process.
//
// The canvas is seeded deliberately rather than relying on a template: a
// freshly-opened project is a single uniform artboard colour, so the test
// creates one high-contrast rect via the real `canvas.createNodes` batch API
// and asserts the painted surface goes from uniform to multi-colour. That
// directly exercises the mutation -> repaint coupling AND the wire-format
// casing fix (the present loop only advances when `frameInfo().frameId` is a
// real number, so a snake/camel regression would hang the poll and fail here).
//
// It also asserts the boot is free of uncaught renderer exceptions and of
// unexpected `console.error` output, so a future regression that merely logs
// its way to a blank screen still fails the gate.
//
// The test drives the already-installed Electron binary through
// `playwright-core`'s `_electron` harness (no browser downloads). It must run
// against a built app: `pnpm build` (main/preload/renderer bundles) and a
// compiled bridge cdylib under `target/<profile>/`. In CI it runs under
// `xvfb-run`; locally any X display works.

import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { _electron as electron } from "playwright-core";

const here = path.dirname(fileURLToPath(import.meta.url));
// tests/e2e/ -> apps/desktop
const appDir = path.resolve(here, "..", "..");

// `console.error` lines that are expected on a clean boot and are tracked by
// their own workstreams. Anything NOT matched here fails the test, so this
// list stays deliberately small and specific.
const ALLOWED_CONSOLE_ERRORS = [
  // Headless software-GL / Vulkan adapter chatter from Chromium + wgpu when
  // no real GPU is present (CI runs under xvfb + lavapipe).
  /GPU|GL |ANGLE|Vulkan|SwiftShader|swiftshader|vk|dri3|VA-API|vaapi/i,
  // Chromium's specific "Automatic fallback to software WebGL has been
  // deprecated" notice. Scoped to "fallback to software" so a genuine app
  // error that merely contains the word "fallback" (e.g. a renderer or cache
  // fallback failure) still fails the gate.
  /fallback to software/i,
  // Autofill DevTools-protocol noise emitted by Chromium when DevTools is not
  // attached. Harmless and unrelated to the app.
  /Autofill\.(enable|setAddresses)/i,
];

function isAllowedConsoleError(text) {
  return ALLOWED_CONSOLE_ERRORS.some((re) => re.test(text));
}

/**
 * Reads the presented canvas back and summarises its pixels so the test can
 * decide whether anything was actually drawn. A blank surface is a single
 * uniform colour (the original P0-2 bug: 100% white); a real frame with our
 * seeded rect has several distinct colours and a large number of pixels that
 * differ from the most common one.
 *
 * Colours are quantised to 4 bits per channel so anti-aliasing fringes don't
 * inflate the distinct-colour count. Sampling strides over the buffer to keep
 * the serialised evaluate payload small.
 */
async function sampleCanvas(canvas) {
  return canvas.evaluate((el) => {
    const c = /** @type {HTMLCanvasElement} */ (el);
    const ctx = c.getContext("2d");
    if (!ctx) return { ok: false, reason: "no 2d context" };
    const w = c.width;
    const h = c.height;
    if (w === 0 || h === 0) return { ok: false, reason: "zero-sized canvas" };
    const data = ctx.getImageData(0, 0, w, h).data;
    const counts = new Map();
    let sampled = 0;
    // Stride in whole pixels; 4 bytes per pixel.
    const stride = 4 * 4;
    for (let i = 0; i + 3 < data.length; i += stride) {
      const key =
        ((data[i] >> 4) << 8) | ((data[i + 1] >> 4) << 4) | (data[i + 2] >> 4);
      counts.set(key, (counts.get(key) ?? 0) + 1);
      sampled += 1;
    }
    let dominant = 0;
    for (const n of counts.values()) if (n > dominant) dominant = n;
    return {
      ok: true,
      width: w,
      height: h,
      sampled,
      distinct: counts.size,
      dominant,
      nonDominant: sampled - dominant,
    };
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

test("boots to HomePage and presents a seeded rect on the editor canvas", async (t) => {
  // GitHub hosted runners can't use Chromium's setuid sandbox; opt into
  // `--no-sandbox` there via env so the local run stays sandboxed (the
  // realistic desktop config) while CI still boots.
  const extraArgs =
    process.env.KCREATE_E2E_NO_SANDBOX === "1" ? ["--no-sandbox"] : [];
  const app = await electron.launch({
    args: [".", ...extraArgs],
    cwd: appDir,
    env: {
      ...process.env,
      // The bridge cdylib path is profile-relative; CI builds debug.
      KCREATE_BRIDGE_PROFILE: process.env.KCREATE_BRIDGE_PROFILE ?? "debug",
    },
  });
  t.after(async () => {
    await app.close();
  });

  const consoleErrors = [];
  const pageErrors = [];

  const win = await app.firstWindow();
  win.on("console", (msg) => {
    if (msg.type() === "error") consoleErrors.push(msg.text());
  });
  win.on("pageerror", (err) => {
    pageErrors.push(err.stack ?? String(err));
  });

  await win.waitForLoadState("domcontentloaded");

  // P0-1: HomePage rendered. The "App / Website UI" create card is always
  // present in the "Create new" section.
  const appUiCard = win.getByTestId("kcreate-create-card-app-ui");
  await appUiCard.waitFor({ state: "visible", timeout: 30_000 });

  // Open the editor (dialog-free scratch project) and wait for the present
  // surface to mount.
  await appUiCard.click();
  const canvas = win.getByTestId("kcreate-canvas-surface");
  await canvas.waitFor({ state: "visible", timeout: 30_000 });

  // Guard the wire-format casing regression directly: the present path returns
  // a raw napi object whose fields napi-rs camelCases, so `frameId` must be a
  // real number. A snake/camel mismatch makes this `undefined` and freezes the
  // present loop after the first frame.
  const frameInfo = await win.evaluate(async () => {
    const info = await window.kcreate.renderer.frameInfo();
    return { frameIdType: typeof info?.frameId, frameId: info?.frameId };
  });
  assert.equal(
    frameInfo.frameIdType,
    "number",
    `renderer.frameInfo().frameId must be numeric (camelCase wire shape); got ${JSON.stringify(frameInfo)}`,
  );

  // Seed one high-contrast rect centred in the first artboard via the real
  // batch API, so the assertion proves a *document mutation* reaches the
  // canvas rather than relying on template content.
  const seeded = await win.evaluate(async () => {
    const arts = await window.kcreate.artboard.list();
    const a = arts[0];
    if (!a) return { error: "no artboard to seed into" };
    const w = Math.round(a.width * 0.4);
    const h = Math.round(a.height * 0.4);
    const x = Math.round(a.x + (a.width - w) / 2);
    const y = Math.round(a.y + (a.height - h) / 2);
    const ids = await window.kcreate.canvas.createNodes([
      {
        kind: "rect",
        parent: null,
        x,
        y,
        w,
        h,
        // Burnt orange — far from the white artboard and the canvas backdrop.
        fill: { kind: "solid", r: 0.9, g: 0.317, b: 0, a: 1 },
        name: "smoke-rect",
      },
    ]);
    return { ids };
  });
  assert.ok(
    seeded.ids && seeded.ids.length === 1,
    `failed to seed rect via canvas.createNodes: ${JSON.stringify(seeded)}`,
  );

  // P0-2: the canvas must repaint with the seeded content. Frames are produced
  // asynchronously (scene sync -> invalidate -> render -> readback ->
  // putImageData over the rAF loop), so poll until content appears.
  let stats = null;
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    stats = await sampleCanvas(canvas);
    if (stats.ok && stats.distinct >= 2 && stats.nonDominant > 500) break;
    await sleep(250);
  }

  assert.ok(stats && stats.ok, `canvas was not sampleable: ${JSON.stringify(stats)}`);
  assert.ok(
    stats.distinct >= 2 && stats.nonDominant > 500,
    `seeded rect never reached the canvas (present loop frozen?): ${JSON.stringify(stats)}`,
  );

  // No uncaught renderer exceptions during boot.
  assert.equal(
    pageErrors.length,
    0,
    `uncaught renderer errors during boot:\n${pageErrors.join("\n---\n")}`,
  );

  // No unexpected console.error output.
  const unexpected = consoleErrors.filter((e) => !isAllowedConsoleError(e));
  assert.equal(
    unexpected.length,
    0,
    `unexpected console errors during boot:\n${unexpected.join("\n---\n")}`,
  );
});
