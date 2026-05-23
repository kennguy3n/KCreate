// PreflightPanel — run print-readiness checks against the open
// document and surface issues grouped by severity (error / warning /
// info). Bound to `window.kcreate.preflight` in `preload.ts`.

import { useCallback, useMemo, useState } from "react";

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
  requireBleedMm: 3,
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
          label="Bleed (mm)"
          value={opts.requireBleedMm}
          onChange={(v) => setOpts({ ...opts, requireBleedMm: v })}
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
          onChange={(v) =>
            // Stored as a fraction; users think in percent. 300% =
            // GRACoL / SWOP default; 240%-280% for web/newsprint.
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
  onChange,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      <input
        type="number"
        value={value}
        onChange={(e) => {
          const n = Number(e.target.value);
          if (Number.isFinite(n)) onChange(n);
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
