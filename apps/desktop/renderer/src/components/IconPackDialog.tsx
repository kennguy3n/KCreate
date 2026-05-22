// IconPackDialog — generate web / iOS / Android / favicon icon packs
// from one or more vector / raster nodes. Triggered from the Export
// panel; results are written to the user-picked output directory.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  IconPackPlatform,
  IconPackRequest,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface IconPackDialogProps {
  /** Nodes to render at each requested icon size. */
  nodeIds: string[];
  onClose: () => void;
  onStatus?: (msg: string | null) => void;
}

export function IconPackDialog({
  nodeIds,
  onClose,
  onStatus,
}: IconPackDialogProps): JSX.Element {
  const [platforms, setPlatforms] = useState<IconPackPlatform[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [outputDir, setOutputDir] = useState("");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<string[] | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const list = await window.kcreate.iconPack.builtInPlatforms();
        setPlatforms(list);
        // Pre-select web by default.
        setSelected(
          new Set(
            list.filter((p) => p.name === "web").map((p) => p.name),
          ),
        );
      } catch (e) {
        onStatus?.(`icon pack: ${errMsg(e)}`);
      }
    })();
  }, [onStatus]);

  const generate = useCallback(async () => {
    if (selected.size === 0) {
      onStatus?.("Pick at least one platform.");
      return;
    }
    if (!outputDir.trim()) {
      onStatus?.("Enter an output directory.");
      return;
    }
    setBusy(true);
    onStatus?.("Icon pack: generating…");
    try {
      const req: IconPackRequest = {
        nodeIds,
        platforms: platforms.filter((p) => selected.has(p.name)),
        outputDir: outputDir.trim(),
      };
      const written = await window.kcreate.iconPack.generate(req);
      setResults(written);
      onStatus?.(`Icon pack: ${written.length} files written.`);
    } catch (e) {
      onStatus?.(`Icon pack failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [nodeIds, onStatus, outputDir, platforms, selected]);

  const totalSizes = useMemo(
    () =>
      platforms
        .filter((p) => selected.has(p.name))
        .reduce((acc, p) => acc + p.sizes.length, 0),
    [platforms, selected],
  );

  return (
    <div style={overlayStyle} role="dialog" aria-modal>
      <div style={dialogStyle}>
        <header style={{ display: "flex", alignItems: "center" }}>
          <h2 style={{ margin: 0, fontSize: 16 }}>Generate icon pack</h2>
          <button
            type="button"
            onClick={onClose}
            style={{
              marginLeft: "auto",
              background: "none",
              border: "none",
              color: colors.textMuted,
              cursor: "pointer",
              fontSize: 18,
            }}
            aria-label="Close"
          >
            ×
          </button>
        </header>
        <p style={{ margin: 0, fontSize: 12, color: colors.textMuted }}>
          {nodeIds.length} node(s) selected · {totalSizes} icon size(s)
          will be rendered.
        </p>
        <section style={{ display: "flex", flexDirection: "column", gap: 6 }}>
          {platforms.map((p) => (
            <label
              key={p.name}
              style={{
                display: "flex",
                alignItems: "center",
                gap: spacing.sm,
                padding: spacing.sm,
                border: `1px solid ${colors.border}`,
                borderRadius: radius.card,
                cursor: "pointer",
              }}
            >
              <input
                type="checkbox"
                checked={selected.has(p.name)}
                onChange={(e) => {
                  setSelected((prev) => {
                    const next = new Set(prev);
                    if (e.target.checked) next.add(p.name);
                    else next.delete(p.name);
                    return next;
                  });
                }}
              />
              <div style={{ flex: 1 }}>
                <div
                  style={{
                    fontSize: 13,
                    fontWeight: 600,
                    textTransform: "capitalize",
                  }}
                >
                  {p.name}
                </div>
                <div style={{ fontSize: 11, color: colors.textMuted }}>
                  {p.sizes.length} sizes: {summarize(p)}
                </div>
              </div>
            </label>
          ))}
        </section>
        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span style={{ fontSize: 11, color: colors.textMuted }}>
            Output directory
          </span>
          <input
            type="text"
            value={outputDir}
            onChange={(e) => setOutputDir(e.target.value)}
            placeholder="/path/to/icons"
            style={inputStyle}
          />
        </label>
        {results === null ? null : (
          <section
            style={{
              maxHeight: 160,
              overflowY: "auto",
              border: `1px solid ${colors.border}`,
              borderRadius: radius.card,
              padding: spacing.sm,
            }}
          >
            <h3 style={{ margin: 0, fontSize: 12 }}>
              Written {results.length} file(s):
            </h3>
            <ul style={{ paddingLeft: 16, margin: "6px 0 0" }}>
              {results.map((p) => (
                <li
                  key={p}
                  style={{ fontSize: 11, color: colors.textMuted }}
                >
                  {p}
                </li>
              ))}
            </ul>
          </section>
        )}
        <footer style={{ display: "flex", gap: spacing.sm }}>
          <button
            type="button"
            onClick={() => {
              void generate();
            }}
            disabled={busy}
            style={{
              padding: `${spacing.sm}px ${spacing.md}px`,
              background: busy ? colors.bgSoft : colors.accent,
              color: busy ? colors.textMuted : colors.textInverse,
              border: "none",
              borderRadius: radius.pill,
              fontWeight: 600,
              fontSize: 12,
              cursor: busy ? "default" : "pointer",
            }}
          >
            {busy ? "Generating…" : "Generate"}
          </button>
          <button
            type="button"
            onClick={onClose}
            style={{
              padding: `${spacing.sm}px ${spacing.md}px`,
              background: "transparent",
              border: `1px solid ${colors.border}`,
              borderRadius: radius.pill,
              fontSize: 12,
              cursor: "pointer",
              color: colors.text,
            }}
          >
            Close
          </button>
        </footer>
      </div>
    </div>
  );
}

function summarize(p: IconPackPlatform): string {
  const seen = new Set<string>();
  for (const s of p.sizes) {
    seen.add(`${s.width}×${s.height}`);
  }
  const list = Array.from(seen);
  if (list.length <= 4) return list.join(", ");
  return `${list.slice(0, 4).join(", ")}, …`;
}

const overlayStyle = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.4)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  zIndex: 1000,
} as const;

const dialogStyle = {
  background: colors.bg,
  borderRadius: radius.card,
  padding: spacing.lg,
  width: "min(560px, 92vw)",
  maxHeight: "90vh",
  overflowY: "auto",
  display: "flex",
  flexDirection: "column",
  gap: spacing.md,
} as const;

const inputStyle = {
  padding: "6px 8px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: 6,
  background: colors.bg,
  color: colors.text,
} as const;

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
