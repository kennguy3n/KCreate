// EncryptionPanel — Phase 8 Block C Task 26.
//
// Drives the SQLCipher passphrase / re-key / recovery surface that
// lives on `window.kcreate.projectEncryption`. Three workflows:
//   1. Encryption disabled (fresh / legacy project): collect a
//      passphrase + confirmation, surface a strength meter, call
//      `enable(passphrase)`.
//   2. Encryption enabled: show the active salt fingerprint +
//      iteration count, offer "Change passphrase" (old + new + confirm)
//      and "Export recovery copy" (passphrase + output path).
//   3. Error reporting for SQLCipher-unavailable builds.
//
// Per the AGENTS.md rule on never modifying generated files and
// never asking confirmation on each panel, this is wired
// end-to-end against the real bridge surface. The status string
// returned from `enable` flows back into local state so the UI
// transitions to the enabled view without a re-fetch.

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import type { EncryptionStatus } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface EncryptionPanelProps {
  /** Status sink, same convention as PreflightPanel. */
  onStatus?: (msg: string | null) => void;
}

interface FormState {
  passphrase: string;
  confirm: string;
}

const EMPTY_FORM: FormState = { passphrase: "", confirm: "" };

const STRENGTH_LABELS = ["Very weak", "Weak", "Fair", "Strong", "Very strong"];

export function EncryptionPanel({
  onStatus,
}: EncryptionPanelProps): JSX.Element {
  const [status, setStatus] = useState<EncryptionStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Enable-encryption form.
  const [enableForm, setEnableForm] = useState<FormState>(EMPTY_FORM);
  const [enableStrength, setEnableStrength] = useState<number | null>(null);

  // Change-passphrase form.
  const [oldPassphrase, setOldPassphrase] = useState<string>("");
  const [newForm, setNewForm] = useState<FormState>(EMPTY_FORM);
  const [newStrength, setNewStrength] = useState<number | null>(null);

  // Recovery export form.
  const [recoveryPassphrase, setRecoveryPassphrase] = useState<string>("");
  const [recoveryPath, setRecoveryPath] = useState<string>("");

  const reload = useCallback(async () => {
    try {
      const next = await window.kcreate.projectEncryption.status();
      setStatus(next);
      setLoadError(null);
    } catch (e) {
      setLoadError(`Load encryption status: ${errMsg(e)}`);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  // Debounce-free strength meter — passphrase length is bounded
  // and the score function is pure, so the round-trip per keystroke
  // is acceptable (sub-millisecond).
  useEffect(() => {
    if (enableForm.passphrase.length === 0) {
      setEnableStrength(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const s = await window.kcreate.projectEncryption.passphraseStrength(
          enableForm.passphrase,
        );
        if (!cancelled) setEnableStrength(s);
      } catch {
        // Strength meter is best-effort; failures don't surface.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [enableForm.passphrase]);

  useEffect(() => {
    if (newForm.passphrase.length === 0) {
      setNewStrength(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const s = await window.kcreate.projectEncryption.passphraseStrength(
          newForm.passphrase,
        );
        if (!cancelled) setNewStrength(s);
      } catch {
        // ignored — best-effort meter only
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [newForm.passphrase]);

  const handleEnable = useCallback(async () => {
    if (enableForm.passphrase.length === 0) {
      setErrorMsg("Passphrase must not be empty.");
      return;
    }
    if (enableForm.passphrase !== enableForm.confirm) {
      setErrorMsg("Passphrase and confirmation do not match.");
      return;
    }
    setBusy("enable");
    setErrorMsg(null);
    try {
      const next = await window.kcreate.projectEncryption.enable(
        enableForm.passphrase,
      );
      setStatus(next);
      setEnableForm(EMPTY_FORM);
      onStatus?.("Project encryption enabled.");
    } catch (e) {
      setErrorMsg(`Enable failed: ${errMsg(e)}`);
    } finally {
      setBusy(null);
    }
  }, [enableForm.confirm, enableForm.passphrase, onStatus]);

  const handleChangePassphrase = useCallback(async () => {
    if (oldPassphrase.length === 0) {
      setErrorMsg("Current passphrase must not be empty.");
      return;
    }
    if (newForm.passphrase.length === 0) {
      setErrorMsg("New passphrase must not be empty.");
      return;
    }
    if (newForm.passphrase !== newForm.confirm) {
      setErrorMsg("New passphrase and confirmation do not match.");
      return;
    }
    setBusy("change-passphrase");
    setErrorMsg(null);
    try {
      await window.kcreate.projectEncryption.changePassphrase(
        oldPassphrase,
        newForm.passphrase,
      );
      setOldPassphrase("");
      setNewForm(EMPTY_FORM);
      onStatus?.("Passphrase rotated.");
      await reload();
    } catch (e) {
      setErrorMsg(`Change passphrase failed: ${errMsg(e)}`);
    } finally {
      setBusy(null);
    }
  }, [
    newForm.confirm,
    newForm.passphrase,
    oldPassphrase,
    onStatus,
    reload,
  ]);

  // Open the OS-native save dialog (filters scoped to `.sqlite` /
  // `.db`, overwrite confirmation enabled) and remember the chosen
  // path. We deliberately do NOT trigger the export here so the
  // user can review the path + re-enter their passphrase before
  // committing — matches the two-step flow of the change-passphrase
  // card above. Cancelling the dialog is a silent no-op.
  const handleChooseRecoveryPath = useCallback(async () => {
    setErrorMsg(null);
    try {
      const picked = await window.kcreate.projectEncryption.pickRecoveryPath();
      if (picked != null) setRecoveryPath(picked);
    } catch (e) {
      setErrorMsg(`Choosing the output path failed: ${errMsg(e)}`);
    }
  }, []);

  const handleExportRecovery = useCallback(async () => {
    if (recoveryPassphrase.length === 0) {
      setErrorMsg("Passphrase must not be empty.");
      return;
    }
    if (recoveryPath.trim().length === 0) {
      setErrorMsg("Choose an output path for the recovery copy.");
      return;
    }
    setBusy("recovery");
    setErrorMsg(null);
    try {
      const written = await window.kcreate.projectEncryption.exportPlaintextRecovery(
        recoveryPassphrase,
        recoveryPath,
      );
      setRecoveryPassphrase("");
      onStatus?.(`Recovery copy written to ${written}.`);
    } catch (e) {
      setErrorMsg(`Recovery export failed: ${errMsg(e)}`);
    } finally {
      setBusy(null);
    }
  }, [onStatus, recoveryPassphrase, recoveryPath]);

  const fingerprint = useMemo(() => {
    if (status == null || !status.enabled || status.salt.length === 0) {
      return null;
    }
    // Salt is base64; show a stable short fingerprint so the user
    // can confirm at a glance which key derivation params their
    // project uses without leaking the salt itself.
    return status.salt.slice(0, 8);
  }, [status]);

  if (loadError != null) {
    return (
      <div style={containerStyle}>
        <Header />
        <div style={errorBannerStyle}>
          {loadError}
          <button
            type="button"
            onClick={() => {
              setLoadError(null);
              void reload();
            }}
            style={textButtonStyle}
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (status == null) {
    return (
      <div style={containerStyle}>
        <Header />
        <small style={{ color: colors.textMuted }}>
          Loading encryption status…
        </small>
      </div>
    );
  }

  return (
    <div style={containerStyle}>
      <Header />

      <section style={cardStyle}>
        <strong style={{ fontSize: 12 }}>
          {status.enabled ? "Encryption enabled" : "Encryption disabled"}
        </strong>
        <small style={{ color: colors.textMuted, fontSize: 11 }}>
          {status.enabled
            ? `SQLCipher with PBKDF2-HMAC-SHA256 (${status.iterations.toLocaleString()} iterations)`
            : `Database is stored in plaintext. Enable to derive a SQLCipher key from a passphrase (${status.iterations.toLocaleString()} PBKDF2 iterations).`}
        </small>
        {fingerprint != null ? (
          <small
            style={{
              fontFamily: "monospace",
              fontSize: 10,
              color: colors.textMuted,
            }}
          >
            Salt fingerprint: {fingerprint}…
          </small>
        ) : null}
      </section>

      {!status.enabled ? (
        <section style={cardStyle}>
          <strong style={{ fontSize: 12 }}>Enable encryption</strong>
          <small style={{ color: colors.textMuted, fontSize: 11 }}>
            Pick a passphrase you will not forget. Without it the
            project cannot be recovered.
          </small>
          <label style={fieldLabelStyle}>
            Passphrase
            <input
              type="password"
              value={enableForm.passphrase}
              onChange={(e) =>
                setEnableForm({ ...enableForm, passphrase: e.target.value })
              }
              autoComplete="new-password"
              disabled={busy != null}
              style={inputStyle}
            />
          </label>
          <StrengthMeter score={enableStrength} />
          <label style={fieldLabelStyle}>
            Confirm passphrase
            <input
              type="password"
              value={enableForm.confirm}
              onChange={(e) =>
                setEnableForm({ ...enableForm, confirm: e.target.value })
              }
              autoComplete="new-password"
              disabled={busy != null}
              style={inputStyle}
            />
          </label>
          <button
            type="button"
            onClick={() => {
              void handleEnable();
            }}
            disabled={busy != null}
            style={primaryButtonStyle(busy === "enable")}
          >
            {busy === "enable" ? "Encrypting…" : "Enable encryption"}
          </button>
        </section>
      ) : (
        <>
          <section style={cardStyle}>
            <strong style={{ fontSize: 12 }}>Change passphrase</strong>
            <small style={{ color: colors.textMuted, fontSize: 11 }}>
              Rotate the passphrase. The salt is unchanged so the
              project file footprint is identical after rotation.
            </small>
            <label style={fieldLabelStyle}>
              Current passphrase
              <input
                type="password"
                value={oldPassphrase}
                onChange={(e) => setOldPassphrase(e.target.value)}
                autoComplete="current-password"
                disabled={busy != null}
                style={inputStyle}
              />
            </label>
            <label style={fieldLabelStyle}>
              New passphrase
              <input
                type="password"
                value={newForm.passphrase}
                onChange={(e) =>
                  setNewForm({ ...newForm, passphrase: e.target.value })
                }
                autoComplete="new-password"
                disabled={busy != null}
                style={inputStyle}
              />
            </label>
            <StrengthMeter score={newStrength} />
            <label style={fieldLabelStyle}>
              Confirm new passphrase
              <input
                type="password"
                value={newForm.confirm}
                onChange={(e) =>
                  setNewForm({ ...newForm, confirm: e.target.value })
                }
                autoComplete="new-password"
                disabled={busy != null}
                style={inputStyle}
              />
            </label>
            <button
              type="button"
              onClick={() => {
                void handleChangePassphrase();
              }}
              disabled={busy != null}
              style={primaryButtonStyle(busy === "change-passphrase")}
            >
              {busy === "change-passphrase"
                ? "Rotating…"
                : "Change passphrase"}
            </button>
          </section>

          <section style={cardStyle}>
            <strong style={{ fontSize: 12 }}>Export recovery copy</strong>
            <small style={{ color: colors.textMuted, fontSize: 11 }}>
              Write an unencrypted SQLite copy of the project
              database to a chosen path. Useful as a one-time
              passphrase-free backup before transferring the project
              to someone else.
            </small>
            <label style={fieldLabelStyle}>
              Passphrase (to unlock)
              <input
                type="password"
                value={recoveryPassphrase}
                onChange={(e) => setRecoveryPassphrase(e.target.value)}
                autoComplete="current-password"
                disabled={busy != null}
                style={inputStyle}
              />
            </label>
            <label style={fieldLabelStyle}>
              Output path
              <div style={{ display: "flex", gap: 6, alignItems: "stretch" }}>
                <input
                  type="text"
                  value={recoveryPath}
                  readOnly
                  placeholder="Click “Choose…” to pick a destination"
                  disabled={busy != null}
                  aria-label="Recovery export path"
                  style={{
                    ...inputStyle,
                    flex: 1,
                    background: colors.bgSoft,
                    cursor: "default",
                  }}
                />
                <button
                  type="button"
                  onClick={() => {
                    void handleChooseRecoveryPath();
                  }}
                  disabled={busy != null}
                  style={secondaryButtonStyle(false)}
                >
                  Choose…
                </button>
              </div>
            </label>
            <button
              type="button"
              onClick={() => {
                void handleExportRecovery();
              }}
              disabled={busy != null || recoveryPath.trim().length === 0}
              style={primaryButtonStyle(busy === "recovery")}
            >
              {busy === "recovery" ? "Exporting…" : "Export recovery copy"}
            </button>
          </section>
        </>
      )}

      {errorMsg != null ? (
        <div style={errorBannerStyle}>
          {errorMsg}
          <button
            type="button"
            onClick={() => setErrorMsg(null)}
            style={textButtonStyle}
            aria-label="Dismiss error"
          >
            ✕
          </button>
        </div>
      ) : null}
    </div>
  );
}

function Header(): JSX.Element {
  return (
    <header style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>Encryption</h3>
      <small style={{ color: colors.textMuted }}>
        SQLCipher key derivation, passphrase rotation, and recovery
        export for the project&apos;s SQLite database.
      </small>
    </header>
  );
}

function StrengthMeter({ score }: { score: number | null }): JSX.Element {
  const fillColors = [
    colors.danger,
    colors.danger,
    colors.warn,
    colors.success,
    colors.success,
  ];
  const safeScore =
    score === null ? null : Math.max(0, Math.min(4, Math.round(score)));
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <div style={{ display: "flex", gap: 2 }}>
        {[0, 1, 2, 3, 4].map((i) => (
          <div
            key={i}
            style={{
              flex: 1,
              height: 4,
              borderRadius: radius.pill,
              background:
                safeScore !== null && i <= safeScore
                  ? fillColors[safeScore]
                  : colors.bgSoft,
              border: `1px solid ${colors.border}`,
            }}
            aria-hidden
          />
        ))}
      </div>
      <small style={{ color: colors.textMuted, fontSize: 10 }}>
        {safeScore !== null ? STRENGTH_LABELS[safeScore] : "\u00A0"}
      </small>
    </div>
  );
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

const containerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.md,
  padding: spacing.md,
  fontSize: 12,
};

const cardStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 4,
  padding: spacing.sm,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
};

const inputStyle: React.CSSProperties = {
  width: "100%",
  padding: 4,
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
  boxSizing: "border-box",
  fontFamily: "monospace",
};

const fieldLabelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  fontSize: 11,
  fontWeight: 600,
  color: colors.textMuted,
};

function primaryButtonStyle(busy: boolean): React.CSSProperties {
  return {
    padding: "6px 10px",
    fontSize: 12,
    fontWeight: 600,
    background: busy ? colors.bgSoft : colors.accent,
    color: busy ? colors.textMuted : colors.textInverse,
    border: `1px solid ${busy ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: busy ? "wait" : "pointer",
  };
}

function secondaryButtonStyle(busy: boolean): React.CSSProperties {
  return {
    padding: "6px 10px",
    fontSize: 12,
    fontWeight: 600,
    background: busy ? colors.accent : "transparent",
    color: busy ? colors.textInverse : colors.accent,
    border: `1px solid ${colors.accent}`,
    borderRadius: radius.pill,
    cursor: busy ? "wait" : "pointer",
  };
}

const errorBannerStyle: React.CSSProperties = {
  background: colors.dangerBgSoft,
  border: `1px solid ${colors.dangerBorder}`,
  color: colors.danger,
  padding: spacing.xs,
  borderRadius: radius.sm,
  display: "flex",
  justifyContent: "space-between",
  gap: 4,
  fontSize: 11,
};

const textButtonStyle: React.CSSProperties = {
  background: "transparent",
  border: "none",
  color: "inherit",
  cursor: "pointer",
  fontSize: 11,
};
