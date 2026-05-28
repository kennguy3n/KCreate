#!/usr/bin/env node
// Pack + sign the staged bundle into a real `.kcz` archive.
//
//   1. Compute a SHA-256 digest of each staged file. Ed25519
//      signing is the actual trust root; the per-file digest is
//      what we feed into the signing input so the host can verify
//      file contents one by one without re-streaming the bundle.
//   2. Ed25519-sign the staging contents with the publisher key
//      defined in `KCREATE_KCHAT_EXT_SIGN_KEY` (an ed25519 secret
//      key in 64-char base64-url-no-pad form). The matching public
//      key must equal `identity.publisherPublicKey` in the
//      manifest; we verify this before signing so a misconfigured
//      key never produces a "signed but unverifiable" bundle.
//   3. Emit a ZIP archive (`dist/kcreate-companion.kcz`) containing
//      `manifest.json`, `panel.js`, and `signature.json` at the
//      bundle root. The host's installer extracts the archive,
//      reads `signature.json`, and verifies the Ed25519 signature
//      against the manifest's `publisherPublicKey`.
import { createHash, createPrivateKey, createPublicKey, sign } from "node:crypto";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { createWriteStream } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const ROOT = resolve(__dirname, "..");
const STAGING = resolve(ROOT, "dist", "staging");
const OUT = resolve(ROOT, "dist", "kcreate-companion.kcz");

/** Recursively list every file under `dir` as `(relPath, absPath)` pairs. */
async function walk(dir, baseDir = dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const out = [];
  for (const entry of entries) {
    const abs = join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...(await walk(abs, baseDir)));
    } else if (entry.isFile()) {
      out.push({ rel: relative(baseDir, abs).replaceAll("\\", "/"), abs });
    }
  }
  out.sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
  return out;
}

/**
 * Decode a base64-url-no-pad blob into a `Buffer`. Throws on
 * non-canonical input so a malformed env var doesn't silently
 * sign the bundle with the wrong key.
 */
export function decodeBase64UrlNoPad(input) {
  if (!/^[A-Za-z0-9_-]+$/u.test(input)) {
    throw new Error("base64url input contains disallowed characters");
  }
  if (input.includes("=")) {
    throw new Error("base64url input must be unpadded");
  }
  const b64 = input.replaceAll("-", "+").replaceAll("_", "/");
  const padded = b64.padEnd(b64.length + ((4 - (b64.length % 4)) % 4), "=");
  return Buffer.from(padded, "base64");
}

/** Encode a `Buffer` as base64-url-no-pad. */
export function encodeBase64UrlNoPad(buf) {
  return Buffer.from(buf)
    .toString("base64")
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/u, "");
}

/**
 * Build the canonical signing input: every file under `staging/`
 * concatenated as `NUL-separated <path>\0<sha-256(bytes)>` pairs,
 * sorted by path. This is what we Ed25519-sign so the host can
 * verify file contents one by one without re-streaming the whole
 * bundle.
 */
export async function buildSigningInput(stagingDir) {
  const files = await walk(stagingDir);
  const parts = [];
  for (const f of files) {
    const bytes = await readFile(f.abs);
    const digest = createHash("sha256").update(bytes).digest("hex");
    parts.push(`${f.rel}\0${digest}`);
  }
  return Buffer.from(parts.join("\n"), "utf8");
}

/**
 * Ed25519-sign `data` with `secretKeyRaw` (32-byte seed). Returns
 * the 64-byte signature.
 *
 * Node's `crypto.sign("ed25519", …)` accepts an Ed25519 KeyObject;
 * we wrap the raw seed in a PKCS#8 DER envelope manually so the
 * script has no native dependency beyond the bundled `node:crypto`.
 */
export function ed25519Sign(data, secretKeyRaw) {
  if (secretKeyRaw.length !== 32) {
    throw new Error(
      `ed25519 secret seed must be 32 bytes, got ${secretKeyRaw.length}`,
    );
  }
  // PKCS#8 envelope for an Ed25519 private key. Fixed prefix plus the
  // 32-byte seed (RFC 8410 §7).
  const PREFIX = Buffer.from(
    "302e020100300506032b657004220420",
    "hex",
  );
  const pkcs8 = Buffer.concat([PREFIX, secretKeyRaw]);
  const key = createPrivateKey({
    key: pkcs8,
    format: "der",
    type: "pkcs8",
  });
  return sign(null, data, key);
}

/**
 * Derive the matching Ed25519 verifying key from a 32-byte seed.
 * Used so the script can refuse to sign with a key that doesn't
 * match the manifest's pinned `publisherPublicKey`.
 */
export function ed25519PublicKey(secretKeyRaw) {
  const PREFIX = Buffer.from(
    "302e020100300506032b657004220420",
    "hex",
  );
  const pkcs8 = Buffer.concat([PREFIX, secretKeyRaw]);
  const priv = createPrivateKey({ key: pkcs8, format: "der", type: "pkcs8" });
  const pub = createPublicKey(priv);
  const der = pub.export({ format: "der", type: "spki" });
  // RFC 8410 §7: Ed25519 SPKI = 12-byte fixed prefix + 32-byte key.
  return der.subarray(der.length - 32);
}

// Pre-computed IEEE 802.3 CRC-32 table. Required by the ZIP file
// format spec; we ship our own polyfill so we stay compatible with
// Node 20 (where `zlib.crc32` is not yet stable).
const CRC32_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let c = i;
    for (let k = 0; k < 8; k += 1) {
      c = (c & 1) === 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[i] = c >>> 0;
  }
  return table;
})();

/** IEEE 802.3 CRC-32 over the given bytes. */
export function crc32(bytes) {
  let crc = 0xffffffff;
  for (let i = 0; i < bytes.length; i += 1) {
    const byte = bytes[i] ?? 0;
    crc = (CRC32_TABLE[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

// Minimal STORE-mode ZIP writer. We only need this one mode (no
// compression) so an external dependency would be overkill; the
// archive is tiny (one bundle.js + manifest + signature.json) and
// the host's installer accepts STORE.
async function writeZip(outPath, entries) {
  const localHeaders = [];
  const centralEntries = [];
  let offset = 0;
  const chunks = [];

  for (const entry of entries) {
    const nameBytes = Buffer.from(entry.path, "utf8");
    const data = entry.bytes;
    const crc = crc32(data);
    const localHeader = Buffer.alloc(30);
    localHeader.writeUInt32LE(0x04034b50, 0); // local file header signature
    localHeader.writeUInt16LE(20, 4); // version needed to extract
    localHeader.writeUInt16LE(0, 6); // gp bit flag
    localHeader.writeUInt16LE(0, 8); // method = store
    localHeader.writeUInt16LE(0, 10); // last mod time
    localHeader.writeUInt16LE(0x21, 12); // last mod date (deterministic)
    localHeader.writeUInt32LE(crc, 14);
    localHeader.writeUInt32LE(data.length, 18);
    localHeader.writeUInt32LE(data.length, 22);
    localHeader.writeUInt16LE(nameBytes.length, 26);
    localHeader.writeUInt16LE(0, 28); // extra field length

    chunks.push(localHeader, nameBytes, data);
    localHeaders.push(offset);

    const centralEntry = Buffer.alloc(46);
    centralEntry.writeUInt32LE(0x02014b50, 0); // central dir signature
    centralEntry.writeUInt16LE(20, 4); // version made by
    centralEntry.writeUInt16LE(20, 6); // version needed to extract
    centralEntry.writeUInt16LE(0, 8); // gp bit flag
    centralEntry.writeUInt16LE(0, 10); // method = store
    centralEntry.writeUInt16LE(0, 12); // last mod time
    centralEntry.writeUInt16LE(0x21, 14); // last mod date
    centralEntry.writeUInt32LE(crc, 16);
    centralEntry.writeUInt32LE(data.length, 20);
    centralEntry.writeUInt32LE(data.length, 24);
    centralEntry.writeUInt16LE(nameBytes.length, 28);
    centralEntry.writeUInt16LE(0, 30); // extra field length
    centralEntry.writeUInt16LE(0, 32); // file comment length
    centralEntry.writeUInt16LE(0, 34); // disk number start
    centralEntry.writeUInt16LE(0, 36); // internal attrs
    centralEntry.writeUInt32LE(0, 38); // external attrs
    centralEntry.writeUInt32LE(offset, 42); // local header offset

    centralEntries.push({ header: centralEntry, name: nameBytes });
    offset += localHeader.length + nameBytes.length + data.length;
  }

  const centralStart = offset;
  for (const ce of centralEntries) {
    chunks.push(ce.header, ce.name);
    offset += ce.header.length + ce.name.length;
  }
  const centralEnd = offset;

  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4); // disk number
  eocd.writeUInt16LE(0, 6); // disk where central dir starts
  eocd.writeUInt16LE(centralEntries.length, 8);
  eocd.writeUInt16LE(centralEntries.length, 10);
  eocd.writeUInt32LE(centralEnd - centralStart, 12);
  eocd.writeUInt32LE(centralStart, 16);
  eocd.writeUInt16LE(0, 20); // comment length
  chunks.push(eocd);

  await mkdir(dirname(outPath), { recursive: true });
  await pipeline(Readable.from(chunks), createWriteStream(outPath));
  return localHeaders;
}

async function main() {
  const stat_ = await stat(STAGING).catch(() => null);
  if (!stat_?.isDirectory()) {
    throw new Error(
      `staging directory missing: ${STAGING}. Run \`pnpm build\` first.`,
    );
  }

  const seed = process.env.KCREATE_KCHAT_EXT_SIGN_KEY;
  if (!seed) {
    throw new Error(
      "KCREATE_KCHAT_EXT_SIGN_KEY env var must be set to a 43-char base64-url-no-pad Ed25519 secret seed (32 bytes).",
    );
  }
  const secretKey = decodeBase64UrlNoPad(seed);
  if (secretKey.length !== 32) {
    throw new Error(
      `signing seed must decode to 32 bytes, got ${secretKey.length}`,
    );
  }

  // Cross-check: the seed's public key must match the manifest's
  // pinned publisher key, otherwise the resulting .kcz would be
  // signed with a key the host won't accept.
  const manifestRaw = await readFile(
    resolve(STAGING, "manifest.json"),
    "utf8",
  );
  const manifest = JSON.parse(manifestRaw);
  const pub = await ed25519PublicKey(secretKey);
  const pubB64 = encodeBase64UrlNoPad(pub);
  if (pubB64 !== manifest.identity.publisherPublicKey) {
    throw new Error(
      `signing key public bytes (${pubB64}) do not match manifest.identity.publisherPublicKey (${manifest.identity.publisherPublicKey}); refusing to sign.`,
    );
  }

  // Sign the canonical file digest list, not the ZIP bytes — the
  // host's verifier mirrors this construction.
  const signingInput = await buildSigningInput(STAGING);
  const signature = await ed25519Sign(signingInput, secretKey);
  const signatureDoc = {
    algorithm: "ed25519",
    publisherPublicKey: manifest.identity.publisherPublicKey,
    signedFileDigest: "sha256",
    signature: encodeBase64UrlNoPad(signature),
  };
  const signatureBytes = Buffer.from(
    `${JSON.stringify(signatureDoc, null, 2)}\n`,
    "utf8",
  );

  // Collect the staging files + the freshly written signature.
  const stagingFiles = await walk(STAGING);
  const entries = [];
  for (const f of stagingFiles) {
    entries.push({ path: f.rel, bytes: await readFile(f.abs) });
  }
  entries.push({ path: "signature.json", bytes: signatureBytes });
  // Deterministic ordering inside the archive.
  entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));

  await writeZip(OUT, entries);
  await writeFile(
    resolve(STAGING, "signature.json"),
    signatureBytes,
    "utf8",
  );
  console.log(`[kcreate-companion] signed bundle written to ${OUT}`);
}

const isEntry = process.argv[1]
  ? resolve(process.argv[1]) === resolve(__filename)
  : false;
if (isEntry) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
