// PreflightPanel — run print-readiness checks against the open
// document and surface issues grouped by severity (error / warning /
// info). Bound to `window.kcreate.preflight` in `preload.ts`.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  PreflightColorSpaceTarget,
  PreflightIssue,
  PreflightOptions,
  PreflightSeverity,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PreflightPanelProps {
  /** Optional list of page UUIDs to limit the check; empty = all. */
  pageIds?: string[];
  /** Bubble up status text so the host can show it in a global bar. */
  onStatus?: (msg: string | null) => void;
  /** Forwarded so clicking an issue's link selects the offending node. */
  onSelectNode?: (nodeId: string) => void;
}

const DEFAULTS: PreflightOptions = {
  targetDpi: 300,
  // 0 = infer floor from `targetColorSpace` (150 for cmyk, 72 for
  // rgb). Matches the Rust-side deny-by-default sentinel; any
  // non-zero value overrides the inference.
  imageDpiFloor: 0,
  requireBleedMm: 3,
  checkBleedAreaCoverage: true,
  allowTransparency: false,
  targetColorSpace: "cmyk",
  targetTotalInkCoverage: 3,
};

export function PreflightPanel({
  pageIds,
  onStatus,
  onSelectNode,
}: PreflightPanelProps): JSX.Element {
  const [opts, setOpts] = useState<PreflightOptions>(DEFAULTS);
  const [issues, setIssues] = useState<PreflightIssue[] | null>(null);
  const [busy, setBusy] = useState(false);

  const run = useCallback(async () => {
    setBusy(true);
    onStatus?.("Preflight: running…");
    try {
      const out = await window.kcreate.preflight.run({
        pageIds: pageIds ?? [],
        options: opts,
      });
      setIssues(out);
      onStatus?.(
        out.length === 0
          ? "Preflight: no issues."
          : `Preflight: ${countBySeverity(out, "error")} errors, ${countBySeverity(out, "warning")} warnings, ${countBySeverity(out, "info")} info.`,
      );
    } catch (e) {
      onStatus?.(`Preflight failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [opts, onStatus, pageIds]);

  const grouped = useMemo(() => groupBySeverity(issues ?? []), [issues]);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
        Print Preflight
      </h3>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1fr 1fr",
          gap: spacing.sm,
        }}
      >
        <NumberField
          label="Target DPI"
          value={opts.targetDpi}
          onChange={(v) => setOpts({ ...opts, targetDpi: v })}
        />
        <NumberField
          label={`DPI floor${opts.imageDpiFloor === 0 ? " (auto)" : ""}`}
          value={opts.imageDpiFloor}
          min={0}
          max={1200}
          onChange={(v) => setOpts({ ...opts, imageDpiFloor: v })}
        />
        <NumberField
          label="Bleed (mm)"
          value={opts.requireBleedMm}
          onChange={(v) => setOpts({ ...opts, requireBleedMm: v })}
        />
        <CheckboxField
          label="Warn on missing bleed coverage"
          checked={opts.checkBleedAreaCoverage}
          onChange={(v) => setOpts({ ...opts, checkBleedAreaCoverage: v })}
        />
        <SelectField
          label="Color space"
          value={opts.targetColorSpace}
          options={[
            { value: "cmyk", label: "CMYK (print)" },
            { value: "rgb", label: "RGB (screen)" },
          ]}
          onChange={(v) =>
            setOpts({
              ...opts,
              targetColorSpace: v as PreflightColorSpaceTarget,
            })
          }
        />
        <CheckboxField
          label="Allow transparency"
          checked={opts.allowTransparency}
          onChange={(v) => setOpts({ ...opts, allowTransparency: v })}
        />
        <NumberField
          label="Max ink (%)"
          value={Math.round(opts.targetTotalInkCoverage * 100)}
          min={50}
          max={500}
          onChange={(v) =>
            // Stored as a fraction; users think in percent. 300% =
            // GRACoL / SWOP default; 240%-280% for web/newsprint.
            // Clamped client-side to [50, 500]%: under 50% would
            // silently disable the check (the Rust validator drops
            // <= 0 caps with no feedback), and over 500% is far
            // outside any documented press tolerance. The bounds
            // are advisory — the user can still type freely and the
            // clamp resolves on commit.
            setOpts({ ...opts, targetTotalInkCoverage: v / 100 })
          }
        />
      </div>
      <button
        type="button"
        onClick={() => {
          void run();
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
        {busy ? "Running…" : "Run preflight"}
      </button>
      {issues === null ? (
        <p style={{ color: colors.textMuted, fontSize: 12, margin: 0 }}>
          Run preflight to check print readiness.
        </p>
      ) : issues.length === 0 ? (
        <p style={{ color: colors.textMuted, fontSize: 12, margin: 0 }}>
          No issues found.
        </p>
      ) : (
        <SummaryStrip issues={issues} />
      )}
      {(["error", "warning", "info"] as const).map((sev) =>
        grouped[sev].length === 0 ? null : (
          <SeveritySection
            key={sev}
            severity={sev}
            issues={grouped[sev]}
            onSelectNode={onSelectNode}
          />
        ),
      )}
    </div>
  );
}

function NumberField({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  /** Optional inclusive lower bound. Set as the HTML `min`
   * attribute (so browsers can surface spinner / validation UX)
   * and clamped at commit time (on blur / Enter), NOT on every
   * keystroke. Per-keystroke clamping made it impossible to type
   * values whose prefix was below `min` (e.g. typing "150" with
   * `min=50` would clamp "1" to "50" mid-stroke). */
  min?: number;
  /** Optional inclusive upper bound; mirrors `min` semantics. */
  max?: number;
  onChange: (v: number) => void;
}): JSX.Element {
  // Local string state so the user can type freely (including
  // intermediate values whose prefix is out of range). The
  // committed numeric `value` prop is the source of truth; this
  // ref-driven mirror only matters while the input is focused.
  const [draft, setDraft] = useState<string>(() => String(value));
  // Re-sync the draft whenever the upstream value changes (e.g. a
  // sibling control resets the options blob). Without this the
  // input would freeze at the user's last keystroke.
  useEffect(() => {
    setDraft(String(value));
  }, [value]);

  const commit = useCallback(() => {
    const n = Number(draft);
    if (!Number.isFinite(n)) {
      // Reject invalid input by snapping back to the committed
      // value; do NOT call `onChange` with NaN.
      setDraft(String(value));
      return;
    }
    let clamped = n;
    if (typeof min === "number") clamped = Math.max(min, clamped);
    if (typeof max === "number") clamped = Math.min(max, clamped);
    if (clamped !== n) {
      // Reflect the clamp visually so the user knows the bound
      // applied.
      setDraft(String(clamped));
    }
    if (clamped !== value) {
      onChange(clamped);
    }
  }, [draft, max, min, onChange, value]);

  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      <input
        type="number"
        value={draft}
        min={min}
        max={max}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            commit();
            (e.target as HTMLInputElement).blur();
          }
        }}
        style={inputStyle}
      />
    </label>
  );
}

function SelectField({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (v: string) => void;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        style={inputStyle}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function CheckboxField({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.xs,
        marginTop: 12,
      }}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
    </label>
  );
}

function SummaryStrip({ issues }: { issues: PreflightIssue[] }): JSX.Element {
  const e = countBySeverity(issues, "error");
  const w = countBySeverity(issues, "warning");
  const i = countBySeverity(issues, "info");
  return (
    <div style={{ display: "flex", gap: spacing.sm, flexWrap: "wrap" }}>
      <Pill kind="error">{e} errors</Pill>
      <Pill kind="warning">{w} warnings</Pill>
      <Pill kind="info">{i} info</Pill>
    </div>
  );
}

function SeveritySection({
  severity,
  issues,
  onSelectNode,
}: {
  severity: PreflightSeverity;
  issues: PreflightIssue[];
  onSelectNode?: (nodeId: string) => void;
}): JSX.Element {
  return (
    <section style={{ display: "flex", flexDirection: "column", gap: 6 }}>
      <h4 style={{ margin: 0, fontSize: 11, color: severityColor(severity) }}>
        {labelForSeverity(severity)} ({issues.length})
      </h4>
      {issues.map((issue, idx) => (
        <IssueCard
          key={`${issue.check}-${idx}`}
          issue={issue}
          onSelectNode={onSelectNode}
        />
      ))}
    </section>
  );
}

function IssueCard({
  issue,
  onSelectNode,
}: {
  issue: PreflightIssue;
  onSelectNode?: (nodeId: string) => void;
}): JSX.Element {
  return (
    <div
      style={{
        background: colors.bg,
        border: `1px solid ${severityColor(issue.severity)}33`,
        borderRadius: radius.card,
        padding: spacing.sm,
        display: "flex",
        flexDirection: "column",
        gap: 4,
      }}
    >
      <div style={{ display: "flex", gap: 6 }}>
        <Pill kind={issue.severity}>{labelForCheck(issue.check)}</Pill>
      </div>
      <p style={{ margin: 0, fontSize: 12, color: colors.text }}>
        {issue.message}
      </p>
      {issue.affected_node_id && onSelectNode ? (
        <button
          type="button"
          onClick={() => onSelectNode(issue.affected_node_id ?? "")}
          style={{
            alignSelf: "flex-start",
            padding: 0,
            background: "none",
            border: "none",
            color: colors.accent,
            fontSize: 11,
            cursor: "pointer",
            textDecoration: "underline",
          }}
        >
          Select node
        </button>
      ) : null}
    </div>
  );
}

function Pill({
  kind,
  children,
}: {
  kind: PreflightSeverity | string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <span
      style={{
        background: severityColor(kind) + "22",
        color: severityColor(kind),
        padding: "1px 8px",
        borderRadius: radius.pill,
        fontSize: 10,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: 0.4,
      }}
    >
      {children}
    </span>
  );
}

const inputStyle = {
  padding: "4px 6px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: 6,
  background: colors.bg,
  color: colors.text,
} as const;

function severityColor(s: string): string {
  if (s === "error") return "#DC2626";
  if (s === "warning") return "#D97706";
  if (s === "info") return "#2563EB";
  return colors.textMuted;
}

function labelForSeverity(s: PreflightSeverity): string {
  if (s === "error") return "Errors";
  if (s === "warning") return "Warnings";
  return "Info";
}

function labelForCheck(id: string): string {
  switch (id) {
    case "bleed_margin":
      return "Bleed margin";
    case "font_embed":
      return "Font embed";
    case "font_glyph_coverage":
      return "Font glyph coverage";
    case "image_resolution":
      return "Image resolution";
    case "color_space":
      return "Color space";
    case "transparency":
      return "Transparency";
    case "page_size":
      return "Page size";
    case "shading":
      return "Shading pattern";
    case "total_ink_coverage":
      return "Total ink coverage";
    default:
      return id;
  }
}

function countBySeverity(
  issues: PreflightIssue[],
  s: PreflightSeverity,
): number {
  return issues.filter((i) => i.severity === s).length;
}

function groupBySeverity(
  issues: PreflightIssue[],
): Record<PreflightSeverity, PreflightIssue[]> {
  return {
    error: issues.filter((i) => i.severity === "error"),
    warning: issues.filter((i) => i.severity === "warning"),
    info: issues.filter((i) => i.severity === "info"),
  };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
