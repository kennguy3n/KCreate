// PreflightPanel — run print-readiness checks against the open
// document, auto-fix the issues that can be fixed, re-run to a clean
// pass, and export a press-ready PDF (bleed + trim/registration marks,
// CMYK, spot separations). Bound to `window.kcreate.preflight` /
// `window.kcreate.export` in `preload.ts`.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  PreflightAutofixRequest,
  PreflightCheckId,
  PreflightColorSpaceTarget,
  PreflightFixResult,
  PreflightIssue,
  PreflightOptions,
  PreflightSeverity,
  PrintReadyExportOutcome,
  PrintReadyOptions,
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
  // 5 mm is the conventional commercial-print safe zone. 0 disables.
  safeMarginMm: 5,
};

// The check classes `preflight_autofix` (phase2.rs) can repair in
// place. Everything else is reported with actionable guidance but
// needs a manual edit (e.g. swap a low-DPI image, nudge text inside
// the safe margin). Keep in lockstep with the Rust match arms.
const AUTO_FIXABLE: ReadonlySet<PreflightCheckId> = new Set<PreflightCheckId>([
  "bleed_margin",
  "bleed_area_empty",
  "color_space",
  "total_ink_coverage",
]);

/** Which long-running action is in flight (drives button disabled state). */
type Pending = null | "run" | "fix" | "export";

interface PageOption {
  id: string;
  name: string;
}

export function PreflightPanel({
  pageIds,
  onStatus,
  onSelectNode,
}: PreflightPanelProps): JSX.Element {
  const [opts, setOpts] = useState<PreflightOptions>(DEFAULTS);
  const [issues, setIssues] = useState<PreflightIssue[] | null>(null);
  const [pending, setPending] = useState<Pending>(null);
  const [appliedFixes, setAppliedFixes] = useState<PreflightFixResult[] | null>(
    null,
  );
  const [pages, setPages] = useState<PageOption[]>([]);
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [projectTitle, setProjectTitle] = useState<string>("");
  const [trimMarks, setTrimMarks] = useState(true);
  const [registrationMarks, setRegistrationMarks] = useState(true);
  const [exportOutcome, setExportOutcome] =
    useState<PrintReadyExportOutcome | null>(null);

  const busy = pending !== null;

  // Enumerate the document's Page nodes (the only valid targets for a
  // print-ready export) and the project title used in the PDF info
  // dict. Best-effort: a failure just leaves the export section in its
  // "no page" state.
  const loadPages = useCallback(async () => {
    try {
      const [tree, info] = await Promise.all([
        window.kcreate.document.getDocumentTree(),
        window.kcreate.document.getProjectInfo(),
      ]);
      const pageNodes: PageOption[] = tree
        .filter((n) => n.nodeType === "Page")
        .map((n) => ({ id: n.id, name: n.name }));
      setPages(pageNodes);
      setSelectedPageId((cur) =>
        cur && pageNodes.some((p) => p.id === cur)
          ? cur
          : (pageNodes[0]?.id ?? null),
      );
      setProjectTitle(info?.name ?? "");
    } catch {
      // Non-fatal — the export controls stay disabled until pages load.
    }
  }, []);

  useEffect(() => {
    void loadPages();
  }, [loadPages]);

  const run = useCallback(async () => {
    setPending("run");
    setAppliedFixes(null);
    onStatus?.("Preflight: running…");
    try {
      const out = await window.kcreate.preflight.run({
        pageIds: pageIds ?? [],
        options: opts,
      });
      setIssues(out);
      onStatus?.(summariseIssues(out));
      void loadPages();
    } catch (e) {
      onStatus?.(`Preflight failed: ${errMsg(e)}`);
    } finally {
      setPending(null);
    }
  }, [opts, onStatus, pageIds, loadPages]);

  const applyFixes = useCallback(
    async (fixes: PreflightCheckId[]) => {
      if (fixes.length === 0) return;
      setPending("fix");
      onStatus?.("Preflight: applying auto-fixes…");
      try {
        const request: PreflightAutofixRequest = {
          pageIds: pageIds ?? [],
          options: opts,
          fixes,
        };
        const outcome = await window.kcreate.preflight.autofix(request);
        setIssues(outcome.issues);
        setAppliedFixes(outcome.applied);
        const remaining = outcome.issues.length;
        onStatus?.(
          remaining === 0
            ? "Preflight: all issues resolved — ready for print."
            : `Preflight: ${remaining} issue${remaining === 1 ? "" : "s"} remain after auto-fix.`,
        );
        void loadPages();
      } catch (e) {
        onStatus?.(`Auto-fix failed: ${errMsg(e)}`);
      } finally {
        setPending(null);
      }
    },
    [opts, onStatus, pageIds, loadPages],
  );

  const exportPrintReady = useCallback(async () => {
    if (!selectedPageId) {
      onStatus?.("Print-ready export: no page node to export.");
      return;
    }
    const defaultName = `kcreate-print-ready-${Date.now()}.pdf`;
    const target = await window.kcreate.runtime.chooseExportTarget(
      "pdf",
      defaultName,
      null,
    );
    if (!target) {
      onStatus?.("Print-ready export: cancelled.");
      return;
    }
    setPending("export");
    setExportOutcome(null);
    onStatus?.("Print-ready: exporting…");
    try {
      const printOptions: PrintReadyOptions = {
        bleedMm: opts.requireBleedMm,
        trimMarks,
        registrationMarks,
        colorMode: opts.targetColorSpace === "cmyk" ? "cmyk" : "rgb",
        title: projectTitle ? `${projectTitle} — print-ready` : "",
      };
      const outcome = await window.kcreate.export.printReady(target, {
        pageId: selectedPageId,
        options: printOptions,
      });
      setExportOutcome(outcome);
      onStatus?.(summariseExport(outcome, target));
    } catch (e) {
      onStatus?.(`Print-ready export failed: ${errMsg(e)}`);
    } finally {
      setPending(null);
    }
  }, [
    selectedPageId,
    opts.requireBleedMm,
    opts.targetColorSpace,
    trimMarks,
    registrationMarks,
    projectTitle,
    onStatus,
  ]);

  const grouped = useMemo(() => groupBySeverity(issues ?? []), [issues]);
  const fixableInIssues = useMemo(() => {
    const set = new Set<PreflightCheckId>();
    for (const issue of issues ?? []) {
      if (AUTO_FIXABLE.has(issue.check)) set.add(issue.check);
    }
    return [...set];
  }, [issues]);

  const clean = issues !== null && issues.length === 0;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
          Print Preflight
        </h3>
        <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
          Check the design against commercial-print standards, fix what can be
          fixed automatically, then export a press-ready PDF.
        </p>
      </div>

      <Section title="Check settings">
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
            min={0}
            max={50}
            onChange={(v) => setOpts({ ...opts, requireBleedMm: v })}
          />
          <NumberField
            label="Safe margin (mm)"
            value={opts.safeMarginMm}
            min={0}
            max={50}
            onChange={(v) => setOpts({ ...opts, safeMarginMm: v })}
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
              // outside any documented press tolerance.
              setOpts({ ...opts, targetTotalInkCoverage: v / 100 })
            }
          />
          <CheckboxField
            label="Warn on missing bleed coverage"
            checked={opts.checkBleedAreaCoverage}
            onChange={(v) => setOpts({ ...opts, checkBleedAreaCoverage: v })}
          />
          <CheckboxField
            label="Allow transparency"
            checked={opts.allowTransparency}
            onChange={(v) => setOpts({ ...opts, allowTransparency: v })}
          />
        </div>
      </Section>

      <button
        type="button"
        onClick={() => {
          void run();
        }}
        disabled={busy}
        style={primaryButton(pending === "run" || !busy)}
      >
        {pending === "run" ? "Running…" : "Run preflight"}
      </button>

      {issues === null ? (
        <p style={{ color: colors.textMuted, fontSize: 12, margin: 0 }}>
          Run preflight to check print readiness.
        </p>
      ) : clean ? (
        <CleanBanner />
      ) : (
        <>
          <SummaryStrip issues={issues} />
          {fixableInIssues.length > 0 ? (
            <AutoFixBar
              count={fixableInIssues.length}
              busy={busy}
              fixing={pending === "fix"}
              onFix={() => {
                void applyFixes(fixableInIssues);
              }}
            />
          ) : null}
        </>
      )}

      {appliedFixes && appliedFixes.length > 0 ? (
        <AppliedFixesCard fixes={appliedFixes} />
      ) : null}

      {(["error", "warning", "info"] as const).map((sev) =>
        grouped[sev].length === 0 ? null : (
          <SeveritySection
            key={sev}
            severity={sev}
            issues={grouped[sev]}
            busy={busy}
            onSelectNode={onSelectNode}
            onFix={(check) => {
              void applyFixes([check]);
            }}
          />
        ),
      )}

      <div style={{ height: 1, background: colors.border, opacity: 0.6 }} />

      <Section title="Print-ready PDF">
        <div
          style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}
        >
          {pages.length === 0 ? (
            <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
              No page to export. Add a Page node (Layout mode) to enable
              print-ready export.
            </p>
          ) : (
            <>
              {pages.length > 1 ? (
                <SelectField
                  label="Page"
                  value={selectedPageId ?? pages[0]?.id ?? ""}
                  options={pages.map((p) => ({
                    value: p.id,
                    label: p.name || "Untitled page",
                  }))}
                  onChange={(v) => setSelectedPageId(v)}
                />
              ) : null}
              <div style={{ display: "flex", gap: spacing.md, flexWrap: "wrap" }}>
                <CheckboxField
                  label="Trim marks"
                  checked={trimMarks}
                  onChange={setTrimMarks}
                  inline
                />
                <CheckboxField
                  label="Registration marks"
                  checked={registrationMarks}
                  onChange={setRegistrationMarks}
                  inline
                />
              </div>
              <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
                {opts.requireBleedMm} mm bleed ·{" "}
                {opts.targetColorSpace === "cmyk" ? "CMYK" : "RGB"} output ·
                spot inks become /Separation plates.
              </p>
              <button
                type="button"
                onClick={() => {
                  void exportPrintReady();
                }}
                disabled={busy || !selectedPageId}
                style={primaryButton(!busy && !!selectedPageId)}
              >
                {pending === "export"
                  ? "Exporting…"
                  : "Export print-ready PDF"}
              </button>
            </>
          )}
          {exportOutcome ? <ExportOutcomeCard outcome={exportOutcome} /> : null}
        </div>
      </Section>
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <h4
        style={{
          margin: 0,
          fontSize: 11,
          fontWeight: 600,
          textTransform: "uppercase",
          letterSpacing: 0.5,
          color: colors.textMuted,
        }}
      >
        {title}
      </h4>
      {children}
    </section>
  );
}

function AutoFixBar({
  count,
  busy,
  fixing,
  onFix,
}: {
  count: number;
  busy: boolean;
  fixing: boolean;
  onFix: () => void;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: spacing.sm,
        padding: spacing.sm,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        background: colors.bgSoft,
      }}
    >
      <span style={{ fontSize: 12, color: colors.text }}>
        {count} issue type{count === 1 ? "" : "s"} can be fixed automatically.
      </span>
      <button
        type="button"
        onClick={onFix}
        disabled={busy}
        style={{
          padding: `4px 12px`,
          background: busy ? colors.bgSoft : colors.accent,
          color: busy ? colors.textMuted : colors.textInverse,
          border: "none",
          borderRadius: radius.pill,
          fontWeight: 600,
          fontSize: 12,
          cursor: busy ? "default" : "pointer",
          whiteSpace: "nowrap",
        }}
      >
        {fixing ? "Fixing…" : `Auto-fix (${count})`}
      </button>
    </div>
  );
}

function CleanBanner(): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.sm,
        padding: spacing.sm,
        border: `1px solid ${colors.success}`,
        borderRadius: radius.card,
        color: colors.success,
        fontSize: 12,
        fontWeight: 600,
      }}
    >
      <span aria-hidden>✓</span>
      All checks passed — this page is ready for print.
    </div>
  );
}

function AppliedFixesCard({
  fixes,
}: {
  fixes: PreflightFixResult[];
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 6,
        padding: spacing.sm,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        background: colors.bg,
      }}
    >
      <span style={{ fontSize: 11, fontWeight: 600, color: colors.textMuted }}>
        Auto-fix results
      </span>
      {fixes.map((f) => (
        <div
          key={f.check}
          style={{ display: "flex", flexDirection: "column", gap: 2 }}
        >
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <Pill kind={f.fixable ? "info" : "warning"}>
              {labelForCheck(f.check)}
            </Pill>
            {f.appliedNodeIds.length > 0 ? (
              <span style={{ fontSize: 10, color: colors.textMuted }}>
                {f.appliedNodeIds.length} layer
                {f.appliedNodeIds.length === 1 ? "" : "s"}
              </span>
            ) : null}
          </div>
          <p style={{ margin: 0, fontSize: 11, color: colors.text }}>
            {f.message}
          </p>
        </div>
      ))}
    </div>
  );
}

function ExportOutcomeCard({
  outcome,
}: {
  outcome: PrintReadyExportOutcome;
}): JSX.Element {
  const [mw, mh] = outcome.mediaBoxMm;
  const [tw, th] = outcome.trimBoxMm;
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 4,
        padding: spacing.sm,
        border: `1px solid ${colors.success}`,
        borderRadius: radius.card,
        background: colors.bg,
      }}
    >
      <span style={{ fontSize: 11, fontWeight: 600, color: colors.success }}>
        Exported press-ready PDF
      </span>
      <DetailRow label="Media box" value={`${mw.toFixed(1)} × ${mh.toFixed(1)} mm`} />
      <DetailRow label="Trim box" value={`${tw.toFixed(1)} × ${th.toFixed(1)} mm`} />
      <DetailRow label="Bleed" value={`${outcome.bleedMm} mm`} />
      <DetailRow label="Color" value={outcome.colorMode.toUpperCase()} />
      <DetailRow
        label="Spot plates"
        value={
          outcome.spotPlates.length > 0
            ? outcome.spotPlates.join(", ")
            : "none"
        }
      />
      <DetailRow label="Size" value={`${outcome.bytesWritten} bytes`} />
    </div>
  );
}

function DetailRow({
  label,
  value,
}: {
  label: string;
  value: string;
}): JSX.Element {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      <span style={{ fontSize: 11, color: colors.text }}>{value}</span>
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
  inline,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  /** Drop the top margin used when laid out in the settings grid. */
  inline?: boolean;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.xs,
        marginTop: inline ? 0 : 12,
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
  busy,
  onSelectNode,
  onFix,
}: {
  severity: PreflightSeverity;
  issues: PreflightIssue[];
  busy: boolean;
  onSelectNode?: (nodeId: string) => void;
  onFix: (check: PreflightCheckId) => void;
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
          busy={busy}
          onSelectNode={onSelectNode}
          onFix={onFix}
        />
      ))}
    </section>
  );
}

function IssueCard({
  issue,
  busy,
  onSelectNode,
  onFix,
}: {
  issue: PreflightIssue;
  busy: boolean;
  onSelectNode?: (nodeId: string) => void;
  onFix: (check: PreflightCheckId) => void;
}): JSX.Element {
  const fixable = AUTO_FIXABLE.has(issue.check);
  return (
    <div
      style={{
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        padding: spacing.sm,
        display: "flex",
        flexDirection: "column",
        gap: 4,
      }}
    >
      <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
        <Pill kind={issue.severity}>{labelForCheck(issue.check)}</Pill>
        {fixable ? (
          <span style={{ fontSize: 10, color: colors.success }}>auto-fixable</span>
        ) : null}
      </div>
      <p style={{ margin: 0, fontSize: 12, color: colors.text }}>
        {issue.message}
      </p>
      <div style={{ display: "flex", gap: spacing.md }}>
        {issue.affected_node_id && onSelectNode ? (
          <button
            type="button"
            onClick={() => onSelectNode(issue.affected_node_id ?? "")}
            style={linkButton}
          >
            Select node
          </button>
        ) : null}
        {fixable ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => onFix(issue.check)}
            style={{ ...linkButton, cursor: busy ? "default" : "pointer" }}
          >
            Auto-fix
          </button>
        ) : null}
      </div>
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
        background: severityBg(kind),
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

const linkButton = {
  alignSelf: "flex-start",
  padding: 0,
  background: "none",
  border: "none",
  color: colors.accent,
  fontSize: 11,
  cursor: "pointer",
  textDecoration: "underline",
} as const;

function primaryButton(enabled: boolean): React.CSSProperties {
  return {
    padding: `${spacing.sm}px ${spacing.md}px`,
    background: enabled ? colors.accent : colors.bgSoft,
    color: enabled ? colors.textInverse : colors.textMuted,
    border: "none",
    borderRadius: radius.pill,
    fontWeight: 600,
    fontSize: 12,
    cursor: enabled ? "pointer" : "default",
  };
}

function severityColor(s: string): string {
  if (s === "error") return colors.danger;
  if (s === "warning") return colors.warn;
  if (s === "info") return colors.info;
  return colors.textMuted;
}

function severityBg(s: string): string {
  if (s === "error") return colors.dangerBgSoft;
  if (s === "warning") return colors.warnBgSoft;
  if (s === "info") return colors.infoBg;
  return colors.bgSoft;
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
    case "overprint_table":
      return "Overprint table";
    case "trapping":
      return "Trapping";
    case "transparency":
      return "Transparency";
    case "page_size":
      return "Page size";
    case "shading":
      return "Shading pattern";
    case "total_ink_coverage":
      return "Total ink coverage";
    case "bleed_area_empty":
      return "Bleed area empty";
    case "safe_margin":
      return "Safe margin";
    case "spot_color_missing":
      return "Spot color missing";
    default:
      return id;
  }
}

function summariseIssues(issues: PreflightIssue[]): string {
  if (issues.length === 0) return "Preflight: no issues.";
  return `Preflight: ${countBySeverity(issues, "error")} errors, ${countBySeverity(
    issues,
    "warning",
  )} warnings, ${countBySeverity(issues, "info")} info.`;
}

function summariseExport(
  outcome: PrintReadyExportOutcome,
  target: string,
): string {
  const [mw, mh] = outcome.mediaBoxMm;
  const spots =
    outcome.spotPlates.length > 0
      ? `, ${outcome.spotPlates.length} spot plate${outcome.spotPlates.length === 1 ? "" : "s"}`
      : "";
  return `Print-ready PDF · ${outcome.bytesWritten} bytes · ${mw.toFixed(
    1,
  )}×${mh.toFixed(1)} mm media · ${outcome.bleedMm} mm bleed${spots} → ${target}`;
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
