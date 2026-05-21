import { useEffect, useState } from "react";

import type {
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

export interface RightPanelProps {
  selected: NodeInfo | null;
  onChange?: (changes: UpdateNodeProps) => void;
  onRequestExport: () => void;
}

export function RightPanel({
  selected,
  onChange,
  onRequestExport,
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
          <PropertiesPanel node={selected} onChange={onChange} />
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
}: {
  node: NodeInfo | null;
  onChange?: (changes: UpdateNodeProps) => void;
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
    </div>
  );
}

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
