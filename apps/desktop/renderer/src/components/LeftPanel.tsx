import { useState } from "react";

import type { NodeInfo } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export type LeftPanelTab = "pages" | "layers" | "assets";

export interface LeftPanelProps {
  nodes: NodeInfo[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
}

export function LeftPanel({
  nodes,
  selectedId,
  onSelect,
}: LeftPanelProps): JSX.Element {
  const [tab, setTab] = useState<LeftPanelTab>("layers");
  return (
    <aside
      style={{
        width: 240,
        background: colors.bg,
        borderRight: `1px solid ${colors.border}`,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <PanelTabs
        tabs={[
          { id: "pages", label: "Pages" },
          { id: "layers", label: "Layers" },
          { id: "assets", label: "Assets" },
        ]}
        active={tab}
        onChange={setTab}
      />
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: spacing.sm,
        }}
      >
        {tab === "pages" ? (
          <NodeList
            nodes={nodes.filter((n) => n.nodeType === "Page")}
            selectedId={selectedId}
            onSelect={onSelect}
            emptyHint="No pages yet."
          />
        ) : null}
        {tab === "layers" ? (
          <NodeList
            nodes={nodes}
            selectedId={selectedId}
            onSelect={onSelect}
            emptyHint="No layers yet."
            showHierarchy
          />
        ) : null}
        {tab === "assets" ? (
          <EmptyHint>Drop or import images, fonts, and palettes here.</EmptyHint>
        ) : null}
      </div>
    </aside>
  );
}

function PanelTabs<T extends string>({
  tabs,
  active,
  onChange,
}: {
  tabs: ReadonlyArray<{ id: T; label: string }>;
  active: T;
  onChange: (id: T) => void;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        gap: 2,
        padding: `${spacing.sm}px ${spacing.sm}px 0`,
      }}
      role="tablist"
    >
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          aria-selected={active === t.id}
          onClick={() => onChange(t.id)}
          style={{
            flex: 1,
            padding: "6px 8px",
            fontSize: 12,
            fontWeight: 500,
            background: "transparent",
            border: "none",
            color: active === t.id ? colors.accent : colors.textMuted,
            borderBottom: `2px solid ${
              active === t.id ? colors.accent : "transparent"
            }`,
          }}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

function NodeList({
  nodes,
  selectedId,
  onSelect,
  emptyHint,
  showHierarchy = false,
}: {
  nodes: NodeInfo[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  emptyHint: string;
  showHierarchy?: boolean;
}): JSX.Element {
  if (nodes.length === 0) {
    return <EmptyHint>{emptyHint}</EmptyHint>;
  }
  // Build depth map by walking parent_id chain.
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const depthOf = (n: NodeInfo): number => {
    let d = 0;
    let cur: NodeInfo | undefined = n;
    while (cur?.parentId) {
      const parent = byId.get(cur.parentId);
      if (!parent) break;
      d += 1;
      cur = parent;
    }
    return d;
  };
  return (
    <ul
      style={{
        listStyle: "none",
        margin: 0,
        padding: 0,
        display: "flex",
        flexDirection: "column",
        gap: 1,
      }}
    >
      {nodes.map((n) => {
        const depth = showHierarchy ? depthOf(n) : 0;
        const selected = n.id === selectedId;
        return (
          <li key={n.id}>
            <button
              type="button"
              onClick={() => onSelect(selected ? null : n.id)}
              style={{
                width: "100%",
                textAlign: "left",
                padding: `4px 6px 4px ${6 + depth * 12}px`,
                background: selected ? colors.bgSoft : "transparent",
                color: selected ? colors.accent : colors.text,
                border: "none",
                borderRadius: radius.card / 2,
                fontSize: 12,
                fontWeight: selected ? 600 : 400,
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                gap: 6,
              }}
            >
              <span
                style={{
                  fontSize: 10,
                  color: colors.textMuted,
                  textTransform: "uppercase",
                  letterSpacing: 0.4,
                }}
              >
                {nodeTypeAbbrev(n.nodeType)}
              </span>
              <span style={{ flex: 1 }}>{n.name}</span>
              {!n.visible ? (
                <span style={{ fontSize: 10, color: colors.textMuted }}>
                  hidden
                </span>
              ) : null}
              {n.locked ? (
                <span style={{ fontSize: 10, color: colors.textMuted }}>
                  locked
                </span>
              ) : null}
            </button>
          </li>
        );
      })}
    </ul>
  );
}

function nodeTypeAbbrev(t: string): string {
  switch (t) {
    case "Page":
      return "P";
    case "Artboard":
      return "A";
    case "GroupLayer":
      return "G";
    case "VectorLayer":
      return "V";
    case "RasterLayer":
      return "R";
    case "TextLayer":
      return "T";
    case "ComponentLayer":
      return "C";
    case "LayoutFrame":
      return "L";
    default:
      return "?";
  }
}

function EmptyHint({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div
      style={{
        padding: spacing.md,
        fontSize: 12,
        color: colors.textMuted,
        lineHeight: 1.5,
      }}
    >
      {children}
    </div>
  );
}
