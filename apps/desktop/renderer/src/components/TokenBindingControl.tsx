// TokenBindingControl — Phase 8 Block C Task 22.
//
// Bind a selected node's style properties (fill colour, stroke
// colour, corner radius, stroke width) to design-token names; unbind
// them; or "propagate" a token — re-evaluating every binding pointing
// at it across the document so a colour-token rename instantly
// repaints every linked layer.
//
// Wires `window.kcreate.phase8.bindToken / unbindToken /
// propagateToken / nodeTokenBindings` and `window.kcreate.designTokens.get`.
// The bridge layer enforces type compatibility (e.g. you cannot bind
// `corner_radius` to a colour token) — this UI only surfaces tokens
// of the right kind to avoid round-tripping a bind error.
//
// UX model:
//   * Top: bound-property table (property name, current value preview,
//     bound token name, Unbind button). Re-fetched after every
//     mutation.
//   * Middle: "Add binding" row — property dropdown + filtered token
//     dropdown + Bind button.
//   * Bottom: per-token "Propagate" buttons (re-applies the token's
//     value to every bound property project-wide; useful after
//     `designTokens.set` updates a value).

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import type {
  DesignTokens,
  RgbaColor,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface TokenBindingControlProps {
  /** UUID of the node to bind. `null` collapses the panel to a
   * hint. */
  nodeId: string | null;
  /** Status sink. Same convention as PreflightPanel. */
  onStatus?: (msg: string | null) => void;
}

type PropertyName =
  | "fill"
  | "stroke_color"
  | "corner_radius"
  | "stroke_width";

interface PropertyDescriptor {
  name: PropertyName;
  label: string;
  kind: "color" | "radius" | "spacing";
}

const PROPERTIES: PropertyDescriptor[] = [
  { name: "fill", label: "Fill", kind: "color" },
  { name: "stroke_color", label: "Stroke colour", kind: "color" },
  { name: "corner_radius", label: "Corner radius", kind: "radius" },
  { name: "stroke_width", label: "Stroke width", kind: "spacing" },
];

export function TokenBindingControl({
  nodeId,
  onStatus,
}: TokenBindingControlProps): JSX.Element {
  const [tokens, setTokens] = useState<DesignTokens | null>(null);
  const [bindings, setBindings] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [addProperty, setAddProperty] = useState<PropertyName>("fill");
  const [addToken, setAddToken] = useState<string>("");

  const reloadTokens = useCallback(async (): Promise<void> => {
    try {
      const next = await window.kcreate.designTokens.get();
      setTokens(next);
    } catch (e) {
      setError(`Load design tokens: ${errMsg(e)}`);
    }
  }, []);

  const reloadBindings = useCallback(async (): Promise<void> => {
    if (nodeId == null) {
      setBindings({});
      return;
    }
    try {
      const next = await window.kcreate.phase8.nodeTokenBindings(nodeId);
      setBindings(next);
    } catch (e) {
      setError(`Load bindings: ${errMsg(e)}`);
    }
  }, [nodeId]);

  useEffect(() => {
    void reloadTokens();
  }, [reloadTokens]);

  useEffect(() => {
    void reloadBindings();
  }, [reloadBindings]);

  // Pre-select a token name compatible with the current property
  // selection. Skipping this would leave the picker on a stale,
  // potentially incompatible token from a previous property pick.
  useEffect(() => {
    if (tokens == null) return;
    const available = tokenNamesForKind(
      tokens,
      PROPERTIES.find((p) => p.name === addProperty)?.kind ?? "color",
    );
    if (!available.includes(addToken)) {
      setAddToken(available[0] ?? "");
    }
  }, [addProperty, addToken, tokens]);

  const handleBind = useCallback(async (): Promise<void> => {
    if (nodeId == null) return;
    if (addToken.length === 0) {
      setError("Pick a token to bind to.");
      return;
    }
    setBusy(`bind-${addProperty}`);
    setError(null);
    try {
      await window.kcreate.phase8.bindToken(nodeId, addProperty, addToken);
      onStatus?.(`Bound ${addProperty} to ${addToken}.`);
      await reloadBindings();
    } catch (e) {
      setError(`Bind failed: ${errMsg(e)}`);
    } finally {
      setBusy(null);
    }
  }, [addProperty, addToken, nodeId, onStatus, reloadBindings]);

  const handleUnbind = useCallback(
    async (property: string): Promise<void> => {
      if (nodeId == null) return;
      setBusy(`unbind-${property}`);
      setError(null);
      try {
        await window.kcreate.phase8.unbindToken(nodeId, property);
        onStatus?.(`Unbound ${property}.`);
        await reloadBindings();
      } catch (e) {
        setError(`Unbind failed: ${errMsg(e)}`);
      } finally {
        setBusy(null);
      }
    },
    [nodeId, onStatus, reloadBindings],
  );

  const handlePropagate = useCallback(
    async (tokenName: string): Promise<void> => {
      setBusy(`propagate-${tokenName}`);
      setError(null);
      try {
        const count = await window.kcreate.phase8.propagateToken(tokenName);
        onStatus?.(
          `Propagated ${tokenName} to ${count} node${count === 1 ? "" : "s"}.`,
        );
      } catch (e) {
        setError(`Propagate failed: ${errMsg(e)}`);
      } finally {
        setBusy(null);
      }
    },
    [onStatus],
  );

  // Tokens bound by the current node. Built outside the render
  // loop so we can present per-binding propagate / unbind controls.
  const bindingRows = useMemo(() => {
    const propMap = new Map(PROPERTIES.map((p) => [p.name, p]));
    return Object.entries(bindings).map(([property, tokenName]) => {
      const descriptor = propMap.get(property as PropertyName);
      return {
        property,
        tokenName,
        label: descriptor?.label ?? property,
        kind: descriptor?.kind ?? "color",
        preview: tokens != null ? previewForToken(tokens, tokenName) : null,
      };
    });
  }, [bindings, tokens]);

  if (nodeId == null) {
    return (
      <div
        style={{
          padding: spacing.md,
          fontSize: 12,
          color: colors.textMuted,
        }}
      >
        Select a node to view its design-token bindings.
      </div>
    );
  }

  const availableTokens =
    tokens != null
      ? tokenNamesForKind(
          tokens,
          PROPERTIES.find((p) => p.name === addProperty)?.kind ?? "color",
        )
      : [];

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        padding: spacing.md,
        fontSize: 12,
      }}
    >
      <header style={{ display: "flex", flexDirection: "column", gap: 2 }}>
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
          Token bindings
        </h3>
        <small style={{ color: colors.textMuted }}>
          Bind a style property to a design token so updating the
          token re-paints every linked layer.
        </small>
      </header>

      <section style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <strong style={{ fontSize: 12 }}>Active bindings</strong>
        {bindingRows.length === 0 ? (
          <small style={{ color: colors.textMuted }}>
            No tokens bound on this node.
          </small>
        ) : (
          <ul
            style={{
              listStyle: "none",
              margin: 0,
              padding: 0,
              display: "flex",
              flexDirection: "column",
              gap: 4,
            }}
          >
            {bindingRows.map((row) => (
              <li
                key={row.property}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: spacing.sm,
                  padding: spacing.xs,
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.sm,
                  background: colors.bgSoft,
                }}
              >
                <BindingPreview
                  kind={row.kind as PropertyDescriptor["kind"]}
                  preview={row.preview}
                />
                <div
                  style={{
                    display: "flex",
                    flexDirection: "column",
                    gap: 0,
                    flex: 1,
                    minWidth: 0,
                  }}
                >
                  <strong style={{ fontSize: 11 }}>{row.label}</strong>
                  <small
                    style={{
                      fontSize: 10,
                      color: colors.textMuted,
                      fontFamily: "monospace",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                    title={row.tokenName}
                  >
                    {row.tokenName}
                  </small>
                </div>
                <button
                  type="button"
                  onClick={() => {
                    void handlePropagate(row.tokenName);
                  }}
                  disabled={busy != null}
                  style={chipButtonStyle(
                    busy === `propagate-${row.tokenName}`,
                  )}
                  title="Re-apply the token's current value to every node bound to it"
                >
                  {busy === `propagate-${row.tokenName}`
                    ? "Propagating…"
                    : "Propagate"}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    void handleUnbind(row.property);
                  }}
                  disabled={busy != null}
                  style={chipDangerStyle(busy === `unbind-${row.property}`)}
                >
                  {busy === `unbind-${row.property}` ? "…" : "Unbind"}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 4,
          padding: spacing.sm,
          background: colors.bgSoft,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.md,
        }}
      >
        <strong style={{ fontSize: 12 }}>Add binding</strong>
        <label style={fieldLabelStyle}>
          Property
          <select
            value={addProperty}
            onChange={(e) =>
              setAddProperty(e.target.value as PropertyName)
            }
            style={selectStyle}
          >
            {PROPERTIES.map((p) => (
              <option key={p.name} value={p.name}>
                {p.label}
              </option>
            ))}
          </select>
        </label>
        <label style={fieldLabelStyle}>
          Token
          <select
            value={addToken}
            onChange={(e) => setAddToken(e.target.value)}
            style={selectStyle}
            disabled={availableTokens.length === 0}
          >
            {availableTokens.length === 0 ? (
              <option value="">No compatible tokens defined</option>
            ) : null}
            {availableTokens.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          onClick={() => {
            void handleBind();
          }}
          disabled={
            busy != null ||
            availableTokens.length === 0 ||
            addToken.length === 0
          }
          style={primaryButtonStyle(busy === `bind-${addProperty}`)}
        >
          {busy === `bind-${addProperty}` ? "Binding…" : "Bind"}
        </button>
      </section>

      {error != null ? (
        <div
          style={{
            background: colors.dangerBgSoft,
            border: `1px solid ${colors.dangerBorder}`,
            color: colors.danger,
            padding: spacing.xs,
            borderRadius: radius.sm,
            display: "flex",
            justifyContent: "space-between",
            gap: 4,
            fontSize: 11,
          }}
        >
          <span>{error}</span>
          <button
            type="button"
            onClick={() => setError(null)}
            style={{
              background: "transparent",
              border: "none",
              color: colors.danger,
              cursor: "pointer",
              fontSize: 11,
            }}
            aria-label="Dismiss error"
          >
            ✕
          </button>
        </div>
      ) : null}
    </div>
  );
}

function BindingPreview({
  kind,
  preview,
}: {
  kind: PropertyDescriptor["kind"];
  preview: TokenPreview | null;
}): JSX.Element {
  if (preview == null) {
    return (
      <div
        style={{
          width: 22,
          height: 22,
          borderRadius: radius.sm,
          border: `1px dashed ${colors.border}`,
        }}
        aria-hidden
      />
    );
  }
  if (kind === "color" && preview.kind === "color") {
    return (
      <div
        style={{
          width: 22,
          height: 22,
          borderRadius: radius.sm,
          border: `1px solid ${colors.border}`,
          background: rgbaToCss(preview.value),
        }}
        aria-hidden
      />
    );
  }
  if (preview.kind === "number") {
    return (
      <div
        style={{
          width: 22,
          height: 22,
          borderRadius: radius.sm,
          border: `1px solid ${colors.border}`,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: colors.bg,
          fontSize: 9,
          color: colors.text,
          fontFamily: "monospace",
        }}
        aria-hidden
      >
        {preview.value}
      </div>
    );
  }
  return (
    <div
      style={{
        width: 22,
        height: 22,
        borderRadius: radius.sm,
        border: `1px dashed ${colors.border}`,
      }}
      aria-hidden
    />
  );
}

type TokenPreview =
  | { kind: "color"; value: RgbaColor }
  | { kind: "number"; value: number }
  | { kind: "missing" };

function previewForToken(tokens: DesignTokens, name: string): TokenPreview {
  if (Object.prototype.hasOwnProperty.call(tokens.colors, name)) {
    const v = tokens.colors[name];
    if (v != null) return { kind: "color", value: v };
  }
  if (Object.prototype.hasOwnProperty.call(tokens.radii, name)) {
    const v = tokens.radii[name];
    if (typeof v === "number") return { kind: "number", value: v };
  }
  if (Object.prototype.hasOwnProperty.call(tokens.spacing, name)) {
    const v = tokens.spacing[name];
    if (typeof v === "number") return { kind: "number", value: v };
  }
  return { kind: "missing" };
}

function tokenNamesForKind(
  tokens: DesignTokens,
  kind: PropertyDescriptor["kind"],
): string[] {
  switch (kind) {
    case "color":
      return Object.keys(tokens.colors).sort();
    case "radius":
      return Object.keys(tokens.radii).sort();
    case "spacing":
      return Object.keys(tokens.spacing).sort();
    default: {
      const never: never = kind;
      throw new Error(`unhandled token kind: ${String(never)}`);
    }
  }
}

function rgbaToCss({ r, g, b, a }: RgbaColor): string {
  const c = (v: number): number =>
    Math.max(0, Math.min(255, Math.round(v * 255)));
  return `rgba(${c(r)}, ${c(g)}, ${c(b)}, ${a.toFixed(3)})`;
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

const selectStyle: React.CSSProperties = {
  width: "100%",
  padding: 4,
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
  boxSizing: "border-box",
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

function chipButtonStyle(busy: boolean): React.CSSProperties {
  return {
    padding: "1px 8px",
    fontSize: 10,
    fontWeight: 600,
    background: busy ? colors.accent : "transparent",
    color: busy ? colors.textInverse : colors.accent,
    border: `1px solid ${colors.accent}`,
    borderRadius: radius.pill,
    cursor: busy ? "wait" : "pointer",
  };
}

function chipDangerStyle(busy: boolean): React.CSSProperties {
  return {
    padding: "1px 8px",
    fontSize: 10,
    fontWeight: 600,
    background: busy ? colors.danger : "transparent",
    color: busy ? colors.textInverse : colors.danger,
    border: `1px solid ${colors.danger}`,
    borderRadius: radius.pill,
    cursor: busy ? "wait" : "pointer",
  };
}
