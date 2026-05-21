import { useState } from "react";

import type { NodeInfo } from "../../../shared/scene";
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
  onRequestExport: () => void;
}

export function RightPanel({
  selected,
  onRequestExport,
}: RightPanelProps): JSX.Element {
  const [tab, setTab] = useState<RightPanelTab>("properties");
  return (
    <aside
      style={{
        width: 280,
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
        {tab === "properties" ? <PropertiesPanel node={selected} /> : null}
        {tab === "effects" ? (
          <Hint>
            {selected
              ? "Effects panel will list blur, shadow, glow inputs in Phase 1."
              : "Select a layer to edit effects."}
          </Hint>
        ) : null}
        {tab === "ai" ? (
          <Hint>
            Local-only AI panel: Ask → Preview → Apply → Edit → Undo. Wired
            in Phase 1.
          </Hint>
        ) : null}
        {tab === "export" ? (
          <ExportPanel onRequestExport={onRequestExport} />
        ) : null}
        {tab === "inspect" ? (
          <Hint>
            Read-only inspect: tokens, computed bounds, accessibility checks.
          </Hint>
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

function PropertiesPanel({ node }: { node: NodeInfo | null }): JSX.Element {
  if (!node) {
    return <Hint>Nothing selected. Click a layer to edit its properties.</Hint>;
  }
  return (
    <dl
      style={{
        margin: 0,
        display: "grid",
        gridTemplateColumns: "auto 1fr",
        rowGap: spacing.xs,
        columnGap: spacing.md,
      }}
    >
      <dt style={dtStyle}>id</dt>
      <dd style={ddStyle}>{node.id}</dd>
      <dt style={dtStyle}>type</dt>
      <dd style={ddStyle}>{node.nodeType}</dd>
      <dt style={dtStyle}>name</dt>
      <dd style={ddStyle}>{node.name}</dd>
      <dt style={dtStyle}>parent</dt>
      <dd style={ddStyle}>{node.parentId ?? "—"}</dd>
      <dt style={dtStyle}>children</dt>
      <dd style={ddStyle}>{node.children.length}</dd>
      <dt style={dtStyle}>visible</dt>
      <dd style={ddStyle}>{node.visible ? "yes" : "no"}</dd>
      <dt style={dtStyle}>locked</dt>
      <dd style={ddStyle}>{node.locked ? "yes" : "no"}</dd>
    </dl>
  );
}

const dtStyle: React.CSSProperties = {
  color: colors.textMuted,
  fontWeight: 500,
  margin: 0,
};
const ddStyle: React.CSSProperties = {
  color: colors.text,
  margin: 0,
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace',
  wordBreak: "break-all",
};

function ExportPanel({
  onRequestExport,
}: {
  onRequestExport: () => void;
}): JSX.Element {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <Hint>
        SVG / PNG output flows through the Rust export crate — no network
        round trip.
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
