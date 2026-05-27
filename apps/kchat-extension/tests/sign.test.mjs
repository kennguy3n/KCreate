// Tests for the .kcz bundle signing script. These cover the
// utility primitives exported by `scripts/sign.mjs` — encoding
// helpers, Ed25519 sign/verify round-trip, CRC-32, and the
// per-file digest builder — without actually invoking the full
// `main` (which is gated on the staging dir existing).
import { test } from "node:test";
import assert from "node:assert/strict";
import { createPublicKey, generateKeyPairSync, verify } from "node:crypto";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const SIGN = resolve(ROOT, "scripts/sign.mjs");

const {
  decodeBase64UrlNoPad,
  encodeBase64UrlNoPad,
  ed25519Sign,
  ed25519PublicKey,
  buildSigningInput,
  crc32,
} = await import(`file://${SIGN}`);

test("base64url round-trip", () => {
  const bytes = Buffer.from([0, 1, 2, 250, 251, 252, 253]);
  const encoded = encodeBase64UrlNoPad(bytes);
  assert.match(encoded, /^[A-Za-z0-9_-]+$/u);
  assert.equal(encoded.includes("="), false);
  const decoded = decodeBase64UrlNoPad(encoded);
  assert.deepEqual(Buffer.from(decoded), bytes);
});

test("decodeBase64UrlNoPad rejects padded or otherwise-malformed input", () => {
  // `=` is excluded from the base64url-no-pad alphabet so the
  // disallowed-character guard fires before the explicit
  // unpadded check. Either is fine — the script just must refuse.
  assert.throws(() => decodeBase64UrlNoPad("AAAA="), /disallowed|unpadded/);
});

test("decodeBase64UrlNoPad rejects classic-base64 (+/) characters", () => {
  assert.throws(() => decodeBase64UrlNoPad("AA+/"), /disallowed/);
});

test("crc32 matches the canonical IEEE 802.3 polynomial", () => {
  // Cross-check against the standard known-answer pair for the
  // IEEE 802.3 CRC-32 polynomial (the same one ZIP requires).
  // `123456789` -> 0xCBF43926 is the canonical test vector.
  assert.equal(
    crc32(Buffer.from("123456789", "utf8")),
    0xcbf43926,
    "must implement IEEE 802.3 CRC-32 so the produced ZIP verifies",
  );
  // Empty input -> 0.
  assert.equal(crc32(Buffer.alloc(0)), 0);
});

test("ed25519Sign + ed25519PublicKey round-trip with node:crypto verify", async () => {
  // Generate a fresh keypair using node:crypto, extract the seed
  // and feed it back through our utilities to ensure they agree
  // with the OpenSSL-shaped key the rest of the world uses.
  const { privateKey } = generateKeyPairSync("ed25519");
  const pkcs8 = privateKey.export({ format: "der", type: "pkcs8" });
  const seed = pkcs8.subarray(pkcs8.length - 32);

  const pubRaw = await ed25519PublicKey(seed);
  assert.equal(pubRaw.length, 32);

  const data = Buffer.from("hello kchat", "utf8");
  const sig = await ed25519Sign(data, seed);
  assert.equal(sig.length, 64);

  // Reconstruct the SPKI of the matching public key so node:crypto
  // can verify.
  const SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
  const spki = Buffer.concat([SPKI_PREFIX, pubRaw]);
  const pubKey = createPublicKey({ key: spki, format: "der", type: "spki" });
  assert.equal(verify(null, data, pubKey, sig), true);
});

test("buildSigningInput hashes every staged file deterministically", async () => {
  const dir = await mkdtemp(join(tmpdir(), "kcz-sign-"));
  try {
    await mkdir(join(dir, "nested"), { recursive: true });
    await writeFile(join(dir, "manifest.json"), "{}", "utf8");
    await writeFile(join(dir, "panel.js"), "console.log(1);\n", "utf8");
    await writeFile(join(dir, "nested", "child.txt"), "hi", "utf8");

    const a = await buildSigningInput(dir);
    const b = await buildSigningInput(dir);
    assert.deepEqual(a, b, "must be deterministic on repeat calls");

    // Modifying a single file must change the signing input so the
    // signature would refuse to re-verify post-tamper.
    await writeFile(join(dir, "panel.js"), "console.log(2);\n", "utf8");
    const c = await buildSigningInput(dir);
    assert.notDeepEqual(a, c);

    // The output must mention every file (paths are unique).
    const text = a.toString("utf8");
    assert.ok(text.includes("manifest.json"));
    assert.ok(text.includes("panel.js"));
    assert.ok(text.includes("nested/child.txt"));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
