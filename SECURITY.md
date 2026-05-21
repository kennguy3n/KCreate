# Security Policy

KCreate runs on the user's machine, edits the user's documents, and
spawns local AI sidecars. Security is non-optional.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security reports.

Use [GitHub Security Advisories](https://github.com/kennguy3n/KCreate/security/advisories/new)
to file a private report. Provide:

- A description of the vulnerability and its impact.
- Steps to reproduce.
- Affected versions / commit SHA.
- Suggested remediation if you have one.

We aim to respond on the following timeline:

| Stage             | Target              |
| ----------------- | ------------------- |
| Acknowledgement   | within 48 hours     |
| Initial assessment| within 7 days       |
| Fix or workaround | within 30 days      |

Critical vulnerabilities (RCE, sandbox escape, data exfiltration) take
priority and may receive an out-of-band patch.

## Security principles

KCreate's architecture is built around five principles:

1. **Local-first.** No telemetry, no background uploads, no automatic
   cloud sync. The editor must function fully offline.
2. **Encrypted storage.** Project SQLite databases are encrypted at
   rest (Phase 1+ via SQLCipher). Asset blobs are content-addressed
   and stored in the project folder; they inherit the project's
   encryption key.
3. **Renderer sandbox.** The Electron renderer process runs with
   `contextIsolation: true`, `nodeIntegration: false`, and a narrowly
   scoped `contextBridge` surface. The renderer cannot import native
   code or invoke arbitrary IPC channels.
4. **Strict process separation.** The Rust bridge lives in the
   Electron main process; AI sidecars run in isolated child processes
   with their own resource limits. Crashes are isolated.
5. **No telemetry.** Logging is local-only by default. Crash reports
   are explicit opt-in.

## In scope

We consider the following classes of vulnerability in scope:

- **Renderer sandbox escapes.** Any path from the renderer process to
  arbitrary native code or filesystem access outside the project
  directory.
- **IPC injection.** Crafting an IPC payload that triggers
  unintended behavior in the Rust bridge.
- **Storage bypass.** Reading or writing the encrypted project
  database without the user's key.
- **MCP server escaping loopback.** Any path that lets a remote
  machine reach the loopback-bound MCP server.
- **Operation log / audit tampering.** Any path that lets an attacker
  modify or delete the operation log without leaving traces.
- **Privilege escalation via signed plug-ins** (Phase 2+).

## Out of scope

- Bugs in third-party operating systems, runtimes, GPU drivers, or
  rust-lang itself.
- Denial-of-service via legitimate but expensive editor operations
  (rendering huge artboards, opening malformed PDFs).
- Issues that require physical access to an unlocked machine.

## Disclosure policy

Once a fix is available and shipped, we publish a CVE and a public
advisory. Reporters are credited unless they request anonymity.
