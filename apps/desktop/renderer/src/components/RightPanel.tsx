import { useEffect, useMemo, useState } from "react";

import type {
  FlexLayout,
  GridLayout,
  NodeInfo,
  UpdateNodeProps,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export type RightPanelTab =
  | "properties"
  | "effects"
  | "ai"
  | "export"
  | "inspect"
  | "history";

const TABS: ReadonlyArray<{ id: RightPanelTab; label: string }> = [
  { id: "properties", label: "Properties" },
  { id: "effects", label: "Effects" },
  { id: "ai", label: "AI Assist" },
  { id: "export", label: "Export" },
  { id: "inspect", label: "Inspect" },
  { id: "history", label: "History" },
];

export interface LayoutHandlers {
  setFlex: (nodeId: string, config: FlexLayout) => Promise<void> | void;
  setGrid: (nodeId: string, config: GridLayout) => Promise<void> | void;
  recompute: (nodeId: string) => Promise<void> | void;
  convertToFrame: (nodeId: string) => Promise<void> | void;
}

export interface RightPanelProps {
  selected: NodeInfo | null;
  onChange?: (changes: UpdateNodeProps) => void;
  onRequestExport: () => void;
  layout?: LayoutHandlers;
}

export function RightPanel({
  selected,
  onChange,
  onRequestExport,
  layout,
}: RightPanelProps): JSX.Element {
  const [tab, setTab] = useState<RightPanelTab>("properties");
  return (
    <aside
      style={{
        width: 300,
        background: colors.bg,
        borderLeft: `1px solid ${colors.border}`,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 2,
          padding: `${spacing.sm}px ${spacing.sm}px 0`,
        }}
        role="tablist"
      >
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            style={{
              padding: "4px 10px",
              fontSize: 11,
              fontWeight: 500,
              background: tab === t.id ? colors.bgSoft : "transparent",
              color: tab === t.id ? colors.accent : colors.textMuted,
              border: "none",
              borderRadius: radius.pill,
              cursor: "pointer",
            }}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: spacing.md,
          fontSize: 12,
          color: colors.text,
        }}
      >
        {tab === "properties" ? (
          <PropertiesPanel
            node={selected}
            onChange={onChange}
            layout={layout}
          />
        ) : null}
        {tab === "effects" ? (
          <Hint>
            {selected
              ? "Effects panel will list blur, shadow, glow inputs in Phase 1."
              : "Select a layer to edit effects."}
          </Hint>
        ) : null}
        {tab === "ai" ? (
          <Hint>
            Switch to <b>Image</b> mode for the local AI Assist workflow
            (Ask → Preview → Apply → Edit → Undo).
          </Hint>
        ) : null}
        {tab === "export" ? (
          <ExportTabContent onRequestExport={onRequestExport} />
        ) : null}
        {tab === "inspect" ? (
          <InspectPanel node={selected} />
        ) : null}
        {tab === "history" ? (
          <Hint>
            History timeline (operation log + AI actions) lands with the
            audit crate.
          </Hint>
        ) : null}
      </div>
    </aside>
  );
}

function PropertiesPanel({
  node,
  onChange,
  layout,
}: {
  node: NodeInfo | null;
  onChange?: (changes: UpdateNodeProps) => void;
  layout?: LayoutHandlers;
}): JSX.Element {
  // We keep a local draft of the editable name so the user can type
  // freely without firing a bridge call on every keystroke. The
  // commit fires on blur / Enter, matching the LeftPanel rename UX.
  const [draftName, setDraftName] = useState("");
  useEffect(() => {
    setDraftName(node?.name ?? "");
  }, [node?.id, node?.name]);

  if (!node) {
    return <Hint>Nothing selected. Click a layer to edit its properties.</Hint>;
  }
  const commitName = (): void => {
    if (draftName.trim().length > 0 && draftName !== node.name) {
      onChange?.({ name: draftName.trim() });
    } else {
      setDraftName(node.name);
    }
  };
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
      }}
    >
      <Field label="Name">
        <input
          value={draftName}
          onChange={(e) => setDraftName(e.target.value)}
          onBlur={commitName}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") setDraftName(node.name);
          }}
          style={textInputStyle}
        />
      </Field>
      <Row>
        <ToggleField
          label="Visible"
          value={node.visible}
          onChange={(v) => onChange?.({ visible: v })}
        />
        <ToggleField
          label="Locked"
          value={node.locked}
          onChange={(v) => onChange?.({ locked: v })}
        />
      </Row>
      <hr style={hrStyle} />
      <Field label="Type">
        <Readonly>{node.nodeType}</Readonly>
      </Field>
      <Field label="ID">
        <Readonly mono>{node.id}</Readonly>
      </Field>
      <Field label="Parent">
        <Readonly mono>{node.parentId ?? "—"}</Readonly>
      </Field>
      <Field label="Children">
        <Readonly>{node.children.length}</Readonly>
      </Field>
      {layout ? <LayoutControls node={node} layout={layout} /> : null}
    </div>
  );
}

const DEFAULT_FLEX: FlexLayout = {
  direction: "row",
  spacing: 8,
  padding: { top: 0, right: 0, bottom: 0, left: 0 },
  alignment: "start",
  cross_alignment: "start",
  wrap: false,
};

const DEFAULT_GRID: GridLayout = {
  columns: 3,
  row_gap: 8,
  column_gap: 8,
  padding: { top: 0, right: 0, bottom: 0, left: 0 },
};

/**
 * Per-node layout config picker. Visible when:
 * - the node is a `LayoutFrame` (in which case we render the active
 *   flex/grid controls), or
 * - the node is a `GroupLayer` (in which case we offer a "Convert to
 *   layout frame" promotion).
 */
function LayoutControls({
  node,
  layout,
}: {
  node: NodeInfo;
  layout: LayoutHandlers;
}): JSX.Element | null {
  const stored = useMemo(() => parseLayoutConfig(node), [node]);
  if (node.nodeType === "GroupLayer") {
    return (
      <>
        <hr style={hrStyle} />
        <button
          type="button"
          onClick={() => {
            void layout.convertToFrame(node.id);
          }}
          style={buttonStyle}
        >
          Convert to auto-layout frame
        </button>
      </>
    );
  }
  if (node.nodeType !== "LayoutFrame") {
    return null;
  }
  const [mode, setMode] = [
    stored?.kind ?? "flex",
    (next: "flex" | "grid") => {
      if (next === "flex") {
        void layout.setFlex(node.id, stored?.kind === "flex" ? stored.config : DEFAULT_FLEX);
      } else {
        void layout.setGrid(node.id, stored?.kind === "grid" ? stored.config : DEFAULT_GRID);
      }
      void layout.recompute(node.id);
    },
  ] as const;
  return (
    <>
      <hr style={hrStyle} />
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: spacing.sm,
        }}
      >
        <Field label="Layout mode">
          <SegmentedControl
            value={mode}
            onChange={setMode}
            options={[
              { value: "flex", label: "Flex" },
              { value: "grid", label: "Grid" },
            ]}
          />
        </Field>
        {mode === "flex" ? (
          <FlexControls
            value={stored?.kind === "flex" ? stored.config : DEFAULT_FLEX}
            onCommit={(next) => {
              void (async () => {
                await layout.setFlex(node.id, next);
                await layout.recompute(node.id);
              })();
            }}
          />
        ) : (
          <GridControls
            value={stored?.kind === "grid" ? stored.config : DEFAULT_GRID}
            onCommit={(next) => {
              void (async () => {
                await layout.setGrid(node.id, next);
                await layout.recompute(node.id);
              })();
            }}
          />
        )}
      </div>
    </>
  );
}

type ParsedLayout =
  | { kind: "flex"; config: FlexLayout }
  | { kind: "grid"; config: GridLayout };

function parseLayoutConfig(node: NodeInfo): ParsedLayout | null {
  const raw = node.metadata?.["layout"];
  if (!raw || typeof raw !== "object") {
    return null;
  }
  const mode = (raw as { mode?: string }).mode;
  if (mode === "flex") {
    return { kind: "flex", config: raw as unknown as FlexLayout };
  }
  if (mode === "grid") {
    return { kind: "grid", config: raw as unknown as GridLayout };
  }
  return null;
}

function FlexControls({
  value,
  onCommit,
}: {
  value: FlexLayout;
  onCommit: (next: FlexLayout) => void;
}): JSX.Element {
  const update = (patch: Partial<FlexLayout>): void =>
    onCommit({ ...value, ...patch });
  const updatePadding = (patch: Partial<FlexLayout["padding"]>): void =>
    onCommit({ ...value, padding: { ...value.padding, ...patch } });
  return (
    <>
      <Field label="Direction">
        <SegmentedControl
          value={value.direction}
          onChange={(v) => update({ direction: v })}
          options={[
            { value: "row", label: "Row" },
            { value: "column", label: "Column" },
          ]}
        />
      </Field>
      <Field label="Spacing (px)">
        <NumberInput
          value={value.spacing}
          onCommit={(n) => update({ spacing: n })}
          min={0}
        />
      </Field>
      <PaddingFields
        padding={value.padding}
        onCommit={updatePadding}
      />
      <Field label="Align (main axis)">
        <select
          value={value.alignment}
          onChange={(e) =>
            update({ alignment: e.target.value as FlexLayout["alignment"] })
          }
          style={selectStyle}
        >
          <option value="start">Start</option>
          <option value="center">Center</option>
          <option value="end">End</option>
          <option value="space_between">Space between</option>
          <option value="space_evenly">Space evenly</option>
        </select>
      </Field>
      <Field label="Align (cross axis)">
        <select
          value={value.cross_alignment}
          onChange={(e) =>
            update({
              cross_alignment: e.target
                .value as FlexLayout["cross_alignment"],
            })
          }
          style={selectStyle}
        >
          <option value="start">Start</option>
          <option value="center">Center</option>
          <option value="end">End</option>
          <option value="stretch">Stretch</option>
        </select>
      </Field>
      <ToggleField
        label="Wrap"
        value={value.wrap}
        onChange={(v) => update({ wrap: v })}
      />
    </>
  );
}

function GridControls({
  value,
  onCommit,
}: {
  value: GridLayout;
  onCommit: (next: GridLayout) => void;
}): JSX.Element {
  const update = (patch: Partial<GridLayout>): void =>
    onCommit({ ...value, ...patch });
  const updatePadding = (patch: Partial<GridLayout["padding"]>): void =>
    onCommit({ ...value, padding: { ...value.padding, ...patch } });
  return (
    <>
      <Field label="Columns">
        <NumberInput
          value={value.columns}
          onCommit={(n) => update({ columns: Math.max(1, Math.round(n)) })}
          min={1}
          step={1}
        />
      </Field>
      <Row>
        <Field label="Row gap">
          <NumberInput
            value={value.row_gap}
            onCommit={(n) => update({ row_gap: n })}
            min={0}
          />
        </Field>
        <Field label="Column gap">
          <NumberInput
            value={value.column_gap}
            onCommit={(n) => update({ column_gap: n })}
            min={0}
          />
        </Field>
      </Row>
      <PaddingFields padding={value.padding} onCommit={updatePadding} />
    </>
  );
}

function PaddingFields({
  padding,
  onCommit,
}: {
  padding: { top: number; right: number; bottom: number; left: number };
  onCommit: (patch: Partial<{
    top: number;
    right: number;
    bottom: number;
    left: number;
  }>) => void;
}): JSX.Element {
  return (
    <>
      <Row>
        <Field label="Pad top">
          <NumberInput
            value={padding.top}
            onCommit={(n) => onCommit({ top: n })}
            min={0}
          />
        </Field>
        <Field label="Pad right">
          <NumberInput
            value={padding.right}
            onCommit={(n) => onCommit({ right: n })}
            min={0}
          />
        </Field>
      </Row>
      <Row>
        <Field label="Pad bottom">
          <NumberInput
            value={padding.bottom}
            onCommit={(n) => onCommit({ bottom: n })}
            min={0}
          />
        </Field>
        <Field label="Pad left">
          <NumberInput
            value={padding.left}
            onCommit={(n) => onCommit({ left: n })}
            min={0}
          />
        </Field>
      </Row>
    </>
  );
}

function SegmentedControl<T extends string>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (next: T) => void;
  options: ReadonlyArray<{ value: T; label: string }>;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        background: colors.bgSoft,
        borderRadius: radius.pill,
        padding: 2,
      }}
    >
      {options.map((opt) => (
        <button
          key={opt.value}
          type="button"
          onClick={() => onChange(opt.value)}
          style={{
            flex: 1,
            padding: "4px 8px",
            border: "none",
            background:
              opt.value === value ? colors.accent : "transparent",
            color:
              opt.value === value ? colors.textInverse : colors.textMuted,
            fontSize: 11,
            fontWeight: opt.value === value ? 600 : 500,
            borderRadius: radius.pill,
            cursor: "pointer",
          }}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

function NumberInput({
  value,
  onCommit,
  min,
  step,
}: {
  value: number;
  onCommit: (next: number) => void;
  min?: number;
  step?: number;
}): JSX.Element {
  const [draft, setDraft] = useState<string>(String(value));
  useEffect(() => {
    setDraft(String(value));
  }, [value]);
  return (
    <input
      type="number"
      value={draft}
      min={min}
      step={step ?? "any"}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => {
        const n = Number.parseFloat(draft);
        if (Number.isFinite(n)) {
          onCommit(n);
        } else {
          setDraft(String(value));
        }
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") setDraft(String(value));
      }}
      style={textInputStyle}
    />
  );
}

const selectStyle: React.CSSProperties = {
  ...{
    background: colors.bgSoft,
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: 4,
    padding: "4px 6px",
    fontSize: 12,
    fontFamily: "inherit",
  },
};

const buttonStyle: React.CSSProperties = {
  padding: "6px 14px",
  fontSize: 12,
  fontWeight: 600,
  background: "transparent",
  color: colors.accent,
  border: `1px solid ${colors.accent}`,
  borderRadius: radius.pill,
  cursor: "pointer",
  alignSelf: "flex-start",
};

function InspectPanel({ node }: { node: NodeInfo | null }): JSX.Element {
  if (!node) {
    return <Hint>Select a layer to inspect its computed state.</Hint>;
  }
  return (
    <pre
      style={{
        background: colors.bgSoft,
        padding: spacing.sm,
        margin: 0,
        borderRadius: radius.card / 2,
        fontSize: 11,
        lineHeight: 1.5,
        whiteSpace: "pre-wrap",
        wordBreak: "break-all",
        color: colors.textMuted,
      }}
    >
      {JSON.stringify(node, null, 2)}
    </pre>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 4,
        fontSize: 11,
        color: colors.textMuted,
      }}
    >
      <span>{label}</span>
      {children}
    </label>
  );
}

function Row({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        gap: spacing.md,
      }}
    >
      {children}
    </div>
  );
}

function ToggleField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        fontSize: 12,
        color: colors.text,
        cursor: "pointer",
      }}
    >
      <input
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}

function Readonly({
  children,
  mono = false,
}: {
  children: React.ReactNode;
  mono?: boolean;
}): JSX.Element {
  return (
    <span
      style={{
        color: colors.text,
        fontSize: 12,
        fontFamily: mono
          ? 'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace'
          : undefined,
        wordBreak: "break-all",
      }}
    >
      {children}
    </span>
  );
}

const textInputStyle: React.CSSProperties = {
  background: colors.bgSoft,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: 4,
  padding: "4px 6px",
  fontSize: 12,
  fontFamily: "inherit",
};

const hrStyle: React.CSSProperties = {
  border: "none",
  borderTop: `1px solid ${colors.border}`,
  margin: `${spacing.xs}px 0`,
};

function ExportTabContent({
  onRequestExport,
}: {
  onRequestExport: () => void;
}): JSX.Element {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <Hint>
        Switch to <b>Export</b> mode for the full export panel with PNG /
        SVG / PDF / WebP / JPEG presets and batch export.
      </Hint>
      <button
        type="button"
        onClick={onRequestExport}
        style={{
          padding: "6px 14px",
          fontSize: 12,
          fontWeight: 600,
          background: colors.accent,
          color: colors.textInverse,
          border: `1px solid ${colors.accent}`,
          borderRadius: radius.pill,
          cursor: "pointer",
          alignSelf: "flex-start",
        }}
      >
        Export selected as SVG
      </button>
    </div>
  );
}

function Hint({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <p
      style={{
        margin: 0,
        color: colors.textMuted,
        fontSize: 12,
        lineHeight: 1.5,
      }}
    >
      {children}
    </p>
  );
}
