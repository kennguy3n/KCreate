import { useEffect, useMemo, useRef, useState } from "react";

import { clampTabToAvailable } from "../lib/rightPanelTabs";
import type {
  FillStyle,
  FlexLayout,
  GradientStop,
  GridLayout,
  InspectCode,
  NodeInfo,
  Point2D,
  ProjectInfo,
  RgbaColor,
  SessionLockEntry,
  UpdateNodeProps,
} from "../../../shared/scene";
import { useSessionLocks } from "../hooks/useSessionLocks";
import { colors, radius, spacing } from "../styles/tokens";
import { AccessibilityPanel } from "./AccessibilityPanel";
import { AlignmentToolbar } from "./AlignmentToolbar";
import { ColorSettingsPanel } from "./ColorSettingsPanel";
import { InteractionPanel } from "./InteractionPanel";
import { OpenTypePanel } from "./OpenTypePanel";
import { PreflightPanel } from "./PreflightPanel";
import { PresencePanel } from "./PresencePanel";
import { FiltersPanel } from "./FiltersPanel";
import { ArtifactPublishPanel } from "./ArtifactPublishPanel";
import { ConstraintsPanel } from "./ConstraintsPanel";
import { EncryptionPanel } from "./EncryptionPanel";
import { TextFramePanel } from "./TextFramePanel";
import { TextStylePanel } from "./TextStylePanel";
import { ThemePanel } from "./ThemePanel";
import { TokenBindingControl } from "./TokenBindingControl";
import { Icon, type IconName } from "./Icon";

export type RightPanelTab =
  | "properties"
  | "effects"
  | "ai"
  | "export"
  | "inspect"
  | "history"
  | "accessibility"
  | "interaction"
  | "preflight"
  | "color"
  | "presence"
  | "constraints"
  | "tokens"
  | "theme"
  | "publish"
  | "encryption";

/// One entry in the RightPanel tab strip. `id` is generic so each
/// call site keeps its narrowed `RightPanelTab` discriminant after
/// going through `mkTab` below.
type Tab = { id: RightPanelTab; label: string; icon: IconName };

/// Typed-identity helper used by the `TABS` `useMemo` factory to
/// build conditional tab entries. The parameter type validates the
/// `icon` string against the `IconName` union at compile time. Bare
/// object literals would widen `icon` to `string` (TypeScript only
/// narrows literal-typed properties when the contextual type is
/// propagated, which doesn't happen through a fresh array literal
/// spread), defeating the whole point of `IconName`. The helper
/// restores type-checking without forcing every caller to write
/// `as IconName` casts that silently accept typos.
///
/// Hoisted to module scope (instead of `const mkTab = ...` inside
/// `RightPanel`) for two reasons:
///   1. It's a pure `t => t` identity — it never captures component
///      state, props, or closure refs. Keeping it stable across
///      renders makes the intent obvious.
///   2. The `useMemo` factory references it; defining it inside the
///      component body would create a new function identity each
///      render, and `react-hooks/exhaustive-deps` would (correctly)
///      flag it as a missing dependency. The factory's stable
///      `[showAccessibility, showInteraction, showPreflight, showColor,
///      showTheme]` dep list is the right set; hoisting keeps that
///      list honest.
function mkTab<Id extends RightPanelTab>(
  t: { id: Id; label: string; icon: IconName },
): { id: Id; label: string; icon: IconName } {
  return t;
}

/// Tabs shown by default. Some tabs (Accessibility, Interaction) only
/// appear when the active editor mode calls for them — gated below.
const BASE_TABS: ReadonlyArray<Tab> = [
  { id: "properties", label: "Properties", icon: "sliders-horizontal" },
  { id: "effects", label: "Effects", icon: "sparkles" },
  { id: "ai", label: "AI Assist", icon: "bot" },
  { id: "export", label: "Export", icon: "download" },
  { id: "inspect", label: "Inspect", icon: "code" },
  { id: "history", label: "History", icon: "clock" },
];

export interface LayoutHandlers {
  setFlex: (nodeId: string, config: FlexLayout) => Promise<void> | void;
  setGrid: (nodeId: string, config: GridLayout) => Promise<void> | void;
  recompute: (nodeId: string) => Promise<void> | void;
  convertToFrame: (nodeId: string) => Promise<void> | void;
}

export interface RightPanelProps {
  selected: NodeInfo | null;
  /**
   * Phase D — full multi-selection id list (host-side selection set,
   * not just the single `selected` row). Required by the AlignmentToolbar
   * mounted inside the Properties tab; align/distribute only make sense
   * across a group, so the toolbar gates on `.length >= 2` / `>= 3`.
   * Optional so existing callers (and the in-tree test harness) keep
   * compiling — when omitted the toolbar treats it as an empty selection
   * and stays disabled.
   */
  selectedIds?: string[];
  /**
   * Phase D — host-side callback fired after a successful align /
   * distribute IPC so the document tree can be refreshed. Mirrors the
   * `onApplied` contract on `AlignmentToolbar`. Optional for the same
   * back-compat reason as `selectedIds`.
   */
  onAlignmentApplied?: () => void;
  onChange?: (changes: UpdateNodeProps) => void;
  onRequestExport: () => void;
  layout?: LayoutHandlers;
  /**
   * When set to `"design"` or `"inspect"`, the panel exposes an
   * Accessibility tab driven by the local LLM sidecar. When set to
   * `"prototype"`, the panel exposes an Interaction tab.
   */
  mode?:
    | "design"
    | "vector"
    | "image"
    | "layout"
    | "prototype"
    | "inspect"
    | "export";
  onStatus?: (msg: string | null) => void;
  onSelectNode?: (nodeId: string) => void;
  /** Artboard options used by the Interaction panel's target picker. */
  artboards?: Array<{ id: string; name: string }>;
  /**
   * Full document tree, forwarded to the Interaction panel for the
   * `scroll_to` target picker. Omitted in modes that don't show the
   * Interaction tab.
   */
  tree?: NodeInfo[];
  /** Trigger after Interaction add/remove so the host can refresh state. */
  onInteractionsChanged?: () => void;
  /**
   * G4 — fired after a successful theme/brand-kit apply so the host
   * can re-fetch the document tree, status, and selection. The
   * canvas updates independently via the bridge's scene-sync push;
   * this only resyncs React-side state (layer tree, properties).
   */
  onThemeApplied?: () => void;
  /**
   * Active project, used by the Phase 3 Presence tab. When `null`,
   * the Presence tab still shows (the user can edit display name)
   * but the "Start session" button is disabled.
   */
  project?: ProjectInfo | null;
  /**
   * H1 — programmatic tab focus. The command palette opens a tab
   * (e.g. "theme") by bumping `seq`; the effect below switches the
   * active tab on each new `seq`, clamped to the tabs currently
   * visible for the active `mode`. `seq` (not just the tab id) lets
   * the same tab be re-requested after the user clicked away.
   */
  requestedTab?: { tab: RightPanelTab; seq: number } | null;
}

export function RightPanel({
  selected,
  selectedIds,
  onAlignmentApplied,
  onChange,
  onRequestExport,
  layout,
  mode,
  onStatus,
  onSelectNode,
  artboards,
  tree,
  onInteractionsChanged,
  onThemeApplied,
  project,
  requestedTab,
}: RightPanelProps): JSX.Element {
  const showAccessibility = mode === "design" || mode === "inspect";
  const showInteraction = mode === "prototype";
  const showPreflight = mode === "layout" || mode === "export";
  // Color management lives next to Preflight because the two share
  // the print-bound mental model (working CMYK profile, soft-proof,
  // gamut warning). It's also useful in design mode for picking
  // wide-gamut RGB working spaces (Display P3, Adobe RGB).
  const showColor =
    mode === "layout" || mode === "export" || mode === "design";
  // Theme / Brand Kit is a document-wide restyle. Like Color (its
  // sibling in the colour-management mental model) it only makes
  // sense in the composition modes — design, layout, export — not
  // while editing vector paths, raster images, wiring prototype
  // interactions, or viewing the read-only inspect handoff. Gating
  // it here (rather than rendering it unconditionally) keeps it out
  // of those modes and trims the already-crowded tab strip.
  const showTheme =
    mode === "design" || mode === "layout" || mode === "export";
  // Memoize so the tab strip array identity is stable as long as the
  // mode-derived booleans don't change. Otherwise the spread allocates
  // a fresh array (and new option object literals) on every render,
  // breaking referential equality for any downstream memo.
  //
  // Each conditional entry goes through the module-scope `mkTab`
  // helper (defined above the component) — a typed-identity function
  // whose parameter type validates the `icon` string against the
  // `IconName` union at compile time. Bare object literals would
  // widen `icon` to `string` (TypeScript only narrows literal-typed
  // properties when the contextual type is propagated, which doesn't
  // happen through a fresh array literal spread), defeating the
  // whole point of `IconName`. The helper restores type-checking
  // without forcing every caller to write `as IconName` casts that
  // silently accept typos. `id` is generic so each tab keeps its
  // narrowed `RightPanelTab` discriminant. Hoisting it out of the
  // component body keeps it out of `useMemo`'s captured-deps closure
  // (it's a pure `t => t` identity, identical every render), which
  // silences `react-hooks/exhaustive-deps` without false negatives.
  const TABS = useMemo<ReadonlyArray<Tab>>(
    () => [
      ...BASE_TABS,
      ...(showAccessibility
        ? [mkTab({ id: "accessibility", label: "Accessibility", icon: "eye" })]
        : []),
      ...(showInteraction
        ? [mkTab({ id: "interaction", label: "Interaction", icon: "wand" })]
        : []),
      ...(showPreflight
        ? [mkTab({ id: "preflight", label: "Preflight", icon: "file-text" })]
        : []),
      ...(showColor
        ? [mkTab({ id: "color", label: "Color", icon: "palette" })]
        : []),
      mkTab({ id: "presence", label: "Presence", icon: "users" }),
      // Phase 8 Block C — node-scoped + project-scoped surfaces.
      // Constraints + Tokens are per-selected-node so they only
      // make sense when a node is selected; rendered with a hint
      // otherwise (mirrors how the Properties tab degrades).
      mkTab({ id: "constraints", label: "Constraints", icon: "move" }),
      mkTab({ id: "tokens", label: "Tokens", icon: "variable" }),
      // G4 — Theme / Brand Kit instant restyle. Applies a theme to
      // the whole document in one undoable op (no node selection
      // needed), gated to the composition modes — see `showTheme`.
      ...(showTheme
        ? [mkTab({ id: "theme", label: "Theme", icon: "grid-2x2" })]
        : []),
      mkTab({ id: "publish", label: "Publish", icon: "globe" }),
      mkTab({ id: "encryption", label: "Encryption", icon: "lock" }),
    ],
    [showAccessibility, showInteraction, showPreflight, showColor, showTheme],
  );
  // `selectedIds` is optional; coalesce to a stable empty array so the
  // `?? []` fallback doesn't allocate a fresh `[]` on every render.
  // ThemePanel lists `selectedIds` in the deps of `performApply` (and the
  // callbacks layered on it), so a new reference each render would churn
  // those memoized callbacks for no behavioural change.
  const stableSelectedIds = useMemo<string[]>(
    () => selectedIds ?? [],
    [selectedIds],
  );
  const [tab, setTab] = useState<RightPanelTab>("properties");
  // Clamp the active tab back into the visible strip whenever the
  // editor `mode` transition removes the entry we were on. Without
  // this, switching from `design` (Accessibility visible) to
  // `vector` (Accessibility gone) leaves `tab === "accessibility"`
  // but no matching pill in the strip and no matching `tab === …`
  // branch in the render block — the right panel goes blank until
  // the user clicks any other pill. Devin Review surfaced this on
  // PR #31 round 3 (`RightPanel.tsx:205`); the round-3 reply
  // declined as "pre-existing UX quirk" and noted the proper fix
  // is a clamping pass. This is that fix.
  //
  // The effect only writes when the clamp actually changes value —
  // `clampTabToAvailable` returns `tab` itself when it's still in
  // the strip, so the common case is a single equality check and
  // no state write. Trigger surface is the dep array `[TABS, tab]`:
  //
  //   - `TABS` identity changes once per mode transition (the
  //     `useMemo` above is keyed on the mode-derived booleans), so
  //     mode flips fire the effect exactly once.
  //   - `tab` changes on every user pill click. In that case the
  //     clicked tab is always already in `TABS`, so the clamp is a
  //     no-op single-pass `for` loop with an early `return current`
  //     and no `setTab` call — fast path, no re-render.
  //
  // `tab` must stay in the deps for the React exhaustive-deps rule
  // and to avoid a stale-closure read of the previous tab during
  // back-to-back mode transitions. The no-op guard keeps the
  // tab-click path cheap.
  useEffect(() => {
    const next = clampTabToAvailable(tab, TABS);
    if (next !== tab) setTab(next);
  }, [TABS, tab]);
  // Honour programmatic tab-open requests from the host (command
  // palette). Keyed on the request `seq` so the same tab can be
  // re-opened repeatedly. The requested tab is clamped to the
  // currently-visible strip: callers must already have put the editor
  // in a mode where the tab exists (e.g. the palette forces `design`
  // mode before requesting the mode-gated `theme` tab), but the clamp
  // is a defensive no-op-to-first fallback if the tab is unavailable.
  const requestedSeq = requestedTab?.seq ?? null;
  useEffect(() => {
    if (requestedTab === null || requestedTab === undefined) return;
    setTab(clampTabToAvailable(requestedTab.tab, TABS));
    // Only react to a new `seq`; `requestedTab`/`TABS` are read fresh
    // inside. Including them would re-fire on unrelated re-renders
    // (new object literal) or mode flips (new TABS identity).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestedSeq]);
  // Keep the active pill scrolled into view in the single-row strip.
  //
  // Devin Review on `3007b71` (PR #33) pointed out that with the
  // strip now scrollable, a clamp triggered by a mode transition
  // could leave the newly-active pill off-screen if the user had
  // previously scrolled. Same hazard applies to programmatic
  // `setTab` callers (header pills, future deep-link handlers) and
  // to the case where a user clicks a partially-visible pill near
  // the strip edge — the pill is now highlighted but only half
  // its body is in the viewport.
  //
  // The ref attaches *only* to the pill whose id matches the
  // active `tab` (see the `<button ref={...}>` callback below).
  // That means we never hold references to inactive DOM nodes,
  // and React handles ref re-targeting on each render. `block`
  // and `inline: "nearest"` keep the call cheap when the pill is
  // already fully visible — the browser short-circuits with no
  // scroll. Instant (`behavior: "auto"`) avoids any animation
  // overlap with the mode-transition repaint.
  const activePillRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    activePillRef.current?.scrollIntoView({
      behavior: "auto",
      block: "nearest",
      inline: "nearest",
    });
  }, [tab]);
  // Subscribe to the advisory edit-lock roster so panels can grey
  // out controls (and the right-panel header can render a "Locked
  // by …" pill) when the current selection is held by a remote
  // peer. The hook handles the initial fetch + event subscription
  // + cleanup; outside a session it yields an empty map.
  const { remoteLocks } = useSessionLocks();
  const selectedRemoteLock: SessionLockEntry | null =
    selected !== null ? remoteLocks.get(selected.id) ?? null : null;
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
      {/*
        Tab strip layout — Devin Review PR #31 round 5 surfaced the
        problem (RightPanel.tsx:233): 14 icon+label pills inside a
        300px-wide aside wrapped to 3–4 vertical rows, eating screen
        real estate the panels below should own. Round-5 reply
        sketched three mitigations (scrollable strip, overflow menu,
        icon-only mode); this commit lands the scrollable strip,
        which is the lowest-disruption option:

          - Every pill stays full icon+label, so discoverability for
            the always-on Phase 8 surfaces (Constraints, Tokens,
            Publish, Encryption) survives — those names don't have
            obvious icon glyphs and an icon-only mode would force
            users to hover-and-wait for the tooltip on every glance.
          - One row + horizontal scroll matches the pattern VSCode,
            Figma, and Chrome devtools use for crowded panel tabs,
            so the affordance is already learned by the target user.
          - `flexShrink: 0` on each pill prevents the "all pills
            squished to illegibility" failure mode that a naive
            `whiteSpace: nowrap` parent would otherwise allow when
            the browser tries to compress before scrolling.
          - `scrollbarWidth: "thin"` keeps the native scrollbar
            informative but ~6px tall instead of the default ~12px,
            so it doesn't dominate vertically. WebKit's overlay
            scrollbar already auto-hides; Firefox honours `thin`.

        The lock pill (LockBanner) and content panels below are
        unchanged — only the strip's wrap-vs-scroll behaviour was
        crowding.
      */}
      <div
        style={{
          display: "flex",
          gap: 2,
          padding: `${spacing.sm}px ${spacing.sm}px 0`,
          overflowX: "auto",
          overflowY: "hidden",
          scrollbarWidth: "thin",
        }}
        role="tablist"
      >
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            title={t.label}
            onClick={() => setTab(t.id)}
            // Only the active pill carries the ref — React assigns
            // refs during commit, so on tab change the previous
            // pill's `ref={null}` clears `activePillRef.current`
            // before the new pill's `ref={el => ...}` writes the
            // new node. The `useEffect` on `[tab]` then reads the
            // up-to-date ref and scrolls. No ref Map, no per-pill
            // ref objects.
            ref={t.id === tab ? activePillRef : null}
            style={{
              padding: "4px 10px",
              fontSize: 11,
              fontWeight: 500,
              background: tab === t.id ? colors.bgSoft : "transparent",
              color: tab === t.id ? colors.accent : colors.textMuted,
              border: "none",
              borderRadius: radius.pill,
              cursor: "pointer",
              display: "inline-flex",
              alignItems: "center",
              gap: 4,
              // Keep each pill at its natural width — without
              // `flexShrink: 0` the flex container would try to
              // compress pills before scrolling, producing a row of
              // illegible truncated labels. Forcing pills to their
              // intrinsic size means overflow always becomes a
              // horizontal scroll (the intended affordance) rather
              // than a degraded layout.
              flexShrink: 0,
              whiteSpace: "nowrap",
            }}
          >
            <Icon name={t.icon} size={12} />
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
        {selectedRemoteLock !== null ? (
          <LockBanner lock={selectedRemoteLock} />
        ) : null}
        {tab === "properties" ? (
          <>
            {selectedIds && selectedIds.length >= 2 ? (
              <div
                style={{
                  marginBottom: spacing.sm,
                  borderBottom: `1px solid ${colors.border}`,
                  paddingBottom: spacing.sm,
                }}
              >
                <AlignmentToolbar
                  selectedNodeIds={selectedIds}
                  onApplied={onAlignmentApplied}
                />
              </div>
            ) : null}
            <PropertiesPanel
              node={selected}
              onChange={onChange}
              layout={layout}
              onStatus={onStatus}
              disabled={selectedRemoteLock !== null}
            />
          </>
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
        {tab === "accessibility" && showAccessibility ? (
          <AccessibilityPanel
            onSelectNode={onSelectNode}
            onStatus={onStatus}
            selected={selected}
          />
        ) : null}
        {tab === "interaction" && showInteraction ? (
          <InteractionPanel
            selected={selected}
            artboards={artboards ?? []}
            tree={tree}
            onStatus={onStatus}
            onChanged={onInteractionsChanged}
          />
        ) : null}
        {tab === "preflight" && showPreflight ? (
          <PreflightPanel
            onStatus={onStatus}
            onSelectNode={onSelectNode}
          />
        ) : null}
        {tab === "color" && showColor ? (
          <ColorSettingsPanel onStatus={onStatus} />
        ) : null}
        {tab === "presence" ? (
          <PresencePanel project={project ?? null} onStatus={onStatus} />
        ) : null}
        {tab === "constraints" ? (
          selected !== null ? (
            <ConstraintsPanel
              nodeId={selected.id}
              onStatus={onStatus}
            />
          ) : (
            <Hint>Select a node to edit its resize constraints.</Hint>
          )
        ) : null}
        {tab === "tokens" ? (
          selected !== null ? (
            <TokenBindingControl
              nodeId={selected.id}
              onStatus={onStatus}
            />
          ) : (
            <Hint>Select a node to bind design tokens to its properties.</Hint>
          )
        ) : null}
        {tab === "theme" && showTheme ? (
          <ThemePanel
            onStatus={onStatus}
            onApplied={onThemeApplied}
            selectedIds={stableSelectedIds}
          />
        ) : null}
        {tab === "publish" ? (
          <ArtifactPublishPanel onStatus={onStatus} />
        ) : null}
        {tab === "encryption" ? (
          <EncryptionPanel onStatus={onStatus} />
        ) : null}
      </div>
    </aside>
  );
}

function PropertiesPanel({
  node,
  onChange,
  layout,
  onStatus,
  disabled = false,
}: {
  node: NodeInfo | null;
  onChange?: (changes: UpdateNodeProps) => void;
  layout?: LayoutHandlers;
  onStatus?: (msg: string | null) => void;
  /**
   * Block 8 lock-aware UI: when `true`, every editable control in
   * the panel is rendered in a disabled (greyed-out) state. The
   * caller (`RightPanel`) sets this when the selected node is
   * held by a remote peer's advisory edit lock.
   *
   * We don't suppress `onChange` itself — that would be a
   * defence-in-depth duplicate of the input-level `disabled`
   * attribute, and would also lose us the click-to-focus behaviour
   * users expect even on read-only fields. `disabled` is
   * authoritative.
   */
  disabled?: boolean;
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
    // Use a native `<fieldset disabled>` rather than a `<div>` plus
    // hand-threaded `disabled` props. HTML cascades `disabled` from a
    // `<fieldset disabled>` to every native form element inside it
    // (`<input>`, `<button>`, `<select>`, `<textarea>`) and prevents
    // their click / focus / submit handlers from firing at the
    // platform level — covering the sub-panels (`LayoutControls`,
    // `TextFramePanel`, `OpenTypePanel`, `SegmentedControl`’s native
    // `<button>`s) without us having to thread a `disabled` prop
    // through three component hierarchies and forty inputs by hand.
    // This is the architecturally correct way to express "this group
    // of form controls is disabled" in HTML.
    //
    // The redundant explicit `disabled={disabled}` on Name / Visible
    // / Locked below serves as defence-in-depth and lets each
    // ToggleField paint its own greyed-out cursor / pointer-events
    // CSS without relying on `fieldset[disabled]` styling. Browsers
    // do not propagate styles through `fieldset[disabled]` — only
    // the `disabled` attribute itself — so visual cues (cursor,
    // pointer-events) still want the local prop.
    <fieldset
      disabled={disabled}
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        // Reset the browser's default `<fieldset>` chrome so the
        // element renders identically to the `<div>` it replaces.
        border: "none",
        margin: 0,
        padding: 0,
        minInlineSize: 0,
        // Soften the whole properties body when a remote peer
        // holds the lock on this node.
        opacity: disabled ? 0.55 : 1,
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
          disabled={disabled}
        />
      </Field>
      <Row>
        <ToggleField
          label="Visible"
          value={node.visible}
          onChange={(v) => onChange?.({ visible: v })}
          disabled={disabled}
        />
        <ToggleField
          label="Locked"
          value={node.locked}
          onChange={(v) => onChange?.({ locked: v })}
          disabled={disabled}
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
      {/*
       * FillSection only renders for node types whose paint
       * pipeline actually honours `style.fill`. Today that's
       * `VectorLayer` — see `scene_sync.rs::node_fill`. Future
       * node types that gain fill support should be added here
       * (and to the gradient renderer; today the renderer paints
       * gradients only after we expand it). Keeping the gate
       * here rather than inside `FillSection` means we don't
       * render an "always disabled" panel on raster / text
       * layers where fill simply doesn't apply.
       */}
      {node.nodeType === "VectorLayer" ? (
        <>
          <hr style={hrStyle} />
          <FillSection node={node} onChange={onChange} disabled={disabled} />
        </>
      ) : null}
      {node.nodeType === "TextLayer" ? (
        <>
          <hr style={hrStyle} />
          <TextStylePanel node={node} onStatus={onStatus} />
          <hr style={hrStyle} />
          <TextFramePanel node={node} onStatus={onStatus} />
          <hr style={hrStyle} />
          <OpenTypePanel node={node} onStatus={onStatus} />
        </>
      ) : null}
      {node.nodeType === "RasterLayer" ? (
        <>
          <hr style={hrStyle} />
          <FiltersPanel node={node} onStatus={onStatus} />
        </>
      ) : null}
    </fieldset>
  );
}

/// Banner pinned to the top of the right panel when the selected
/// node is currently held by a remote peer's advisory edit lock.
///
/// The pill renders the holder's peer id (the human-readable
/// display name is not on the lock entry itself — the bridge
/// surfaces it through the presence channel; the renderer can
/// look it up via `useSessionPeers` in a later iteration) plus
/// the acquisition timestamp.
///
/// **Lock semantics**: the lock is *advisory at the protocol
/// layer* — the bridge accepts edit operations from any peer
/// regardless of which one holds the lock, last-write-wins via
/// the LWW resolver. The renderer-side UX, however, treats a
/// remote lock as a strong UI signal: `PropertiesPanel` is
/// rendered inside a `<fieldset disabled>` (see the comment on
/// that element) so every native form control in the body is
/// disabled at the browser level. The user can still override
/// the lock by acting outside the right panel (e.g. dragging on
/// the canvas, which the bridge will accept) — the soft signal
/// is the greyed body + banner + per-input disabled state, not
/// a hard block at the input layer. The acquisition timestamp
/// also gives the user enough context to decide whether the
/// lock is stale.
function LockBanner({ lock }: { lock: SessionLockEntry }): JSX.Element {
  const acquired = formatAcquired(lock.acquiredAt);
  const holderLabel = shortenPeerId(lock.holderPeerId);
  return (
    <div
      role="status"
      aria-live="polite"
      style={{
        background: "#fff7e6",
        border: "1px solid #f5c97c",
        color: "#8a5a00",
        borderRadius: radius.sm,
        padding: `${spacing.xs}px ${spacing.sm}px`,
        marginBottom: spacing.sm,
        fontSize: 11,
        lineHeight: 1.35,
      }}
    >
      <div style={{ fontWeight: 600 }}>Locked by {holderLabel}</div>
      <div style={{ color: "#a17600" }}>since {acquired}</div>
    </div>
  );
}

function formatAcquired(rfc3339: string): string {
  // Best-effort: RFC3339 parses through Date; if it ever fails (a
  // pathological peer payload) fall back to the raw string so the
  // banner still renders something rather than crashing.
  const parsed = new Date(rfc3339);
  if (Number.isNaN(parsed.getTime())) return rfc3339;
  return parsed.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function shortenPeerId(peerId: string): string {
  // Peer ids are base64url-encoded 32-byte Ed25519 public keys —
  // long, ugly, and not human-friendly. Show the first 6 chars so
  // the banner stays readable; the full id is available in the
  // PresencePanel roster for cross-referencing.
  if (peerId.length <= 8) return peerId;
  return `peer ${peerId.slice(0, 6)}…`;
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
 * Per-node fill editor. Wraps the existing `style.fill` field on
 * the document graph; reads via `kcreate.document.nodeFill(id)`
 * and writes via `updateNode({ fill: ... })` (the `fill` field on
 * `UpdateNodeProps` mirrors `kcreate_core::node::FillStyle`
 * verbatim — see `apps/desktop/shared/scene.ts::FillStyle`).
 *
 * Three modes: `none`, `solid`, `gradient`. Solid edits surface
 * an HTML5 colour picker + an alpha slider; gradient edits surface
 * a list of stops with offset + colour, plus add / remove
 * controls and a linear / radial shape toggle. Because the
 * containing `PropertiesPanel` already wraps the whole body in a
 * `<fieldset disabled>` (see the comment near the fieldset open),
 * every native `<input>` / `<button>` here inherits the disabled
 * cascade — but we ALSO take a `disabled` prop and gate the
 * gradient-stop drag handles + add/remove buttons explicitly. The
 * stop drag handles are styled `<div>`s, not `<input>`s, so they
 * sit outside the HTML5 disabled cascade and need the explicit
 * gate to stay correct under remote locks.
 *
 * State management: a local React state holds the editable
 * `FillStyle`; we hydrate it from the bridge on selection change
 * via `kcreate.document.nodeFill`. Every edit is committed
 * immediately to the bridge (no draft / commit step) because
 * the colour picker and the offset slider are continuous inputs
 * — buffering would feel laggy. The operation log dedup in
 * `kcreate_core::operation::OperationLog` collapses adjacent
 * `document_update_node` ops on the same node into one undo step,
 * so the resulting history is still ergonomic.
 */
function FillSection({
  node,
  onChange,
  disabled,
}: {
  node: NodeInfo;
  onChange?: (changes: UpdateNodeProps) => void;
  disabled: boolean;
}): JSX.Element {
  // Hydration state as a discriminated union rather than `FillStyle | null`
  // so the "still fetching" and "fetch failed" cases are
  // distinguishable. The previous `null`-as-both-loading-and-error
  // shape stranded the panel on "Loading…" forever if the bridge
  // ever returned an error for a stable selection (bot finding on
  // RightPanel.tsx:580 — see PR #12 Devin Review thread).
  type HydrateState =
    | { status: "loading" }
    | { status: "loaded"; fill: FillStyle | null }
    | { status: "error"; error: string };
  const [state, setState] = useState<HydrateState>({ status: "loading" });
  // `retryToken` exists so the user can request a re-hydrate from
  // the error UI without us having to invalidate `node.version` /
  // `node.id` to force the effect.
  const [retryToken, setRetryToken] = useState(0);
  // Dependency is `[node.id, node.version, retryToken]`: id changes
  // on selection, `version` increments every time the node is
  // mutated anywhere (bridge writes, undo/redo, future collab
  // events), and `retryToken` lets the error UI re-arm. Previously
  // the effect keyed only on `node.id`, so undo/redo on the same
  // selected node never refired the fetch and the panel showed
  // pre-undo data — see PR #12 Devin Review thread on
  // RightPanel.tsx:549.
  //
  // Stale-while-revalidate: we deliberately do NOT reset the local
  // state to `loading` on every refire. If we already have a
  // `loaded` fill, keep showing it while the new fetch is in flight;
  // only fall back to the spinner when we have nothing better to
  // show (i.e. on the very first hydrate, or after an `error`). This
  // closes the brief "Loading…" flash after undo/redo that the bot
  // flagged on RightPanel.tsx:566 — the optimistic-commit case
  // (`commit()` already wrote into `loaded`) and the undo/redo case
  // (the new value is on its way) are now visually identical.
  useEffect(() => {
    let cancelled = false;
    setState((prev) =>
      prev.status === "loaded" ? prev : { status: "loading" },
    );
    void (async () => {
      try {
        const next = await window.kcreate.document.nodeFill(node.id);
        if (cancelled) {
          return;
        }
        setState({ status: "loaded", fill: next });
      } catch (err) {
        if (cancelled) {
          return;
        }
        setState({
          status: "error",
          error: err instanceof Error ? err.message : String(err),
        });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [node.id, node.version, retryToken]);

  const commit = (next: FillStyle): void => {
    setState({ status: "loaded", fill: next });
    onChange?.({ fill: next });
  };

  if (state.status === "loading") {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
        <SectionLabel>Fill</SectionLabel>
        <Hint>Loading…</Hint>
      </div>
    );
  }

  if (state.status === "error") {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
        <SectionLabel>Fill</SectionLabel>
        <Hint>Couldn’t load fill: {state.error}</Hint>
        <button
          type="button"
          onClick={() => setRetryToken((n) => n + 1)}
          disabled={disabled}
        >
          Retry
        </button>
      </div>
    );
  }

  const fill = state.fill;
  if (fill === null) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
        <SectionLabel>Fill</SectionLabel>
        <Hint>This node does not support fills.</Hint>
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
      <SectionLabel>Fill</SectionLabel>
      <FillKindPicker fill={fill} onCommit={commit} />
      {fill.kind === "solid" ? (
        <SolidFillEditor fill={fill} onCommit={commit} />
      ) : null}
      {fill.kind === "gradient" ? (
        <GradientFillEditor fill={fill} onCommit={commit} disabled={disabled} />
      ) : null}
      <ExtraFillsList
        nodeId={node.id}
        nodeVersion={node.version}
        onChange={onChange}
        disabled={disabled}
      />
    </div>
  );
}

/**
 * Phase 5 Block C Task 17 — multi-fill stack editor.
 *
 * Lists `node.style.extra_fills` (fills layered above the primary
 * `fill` in render order). The user can append a new solid fill,
 * remove an existing one, reorder via Move Up / Move Down, and edit
 * each row via an inline `SolidFillEditor` (gradient stays read-only
 * for now — the renderer already paints gradient extras correctly,
 * but the gradient editor needs a smaller variant before we surface
 * it inline; see RightPanel.tsx:842).
 *
 * Writes go through the same `updateNode` path as the primary fill
 * by sending `extra_fills` on `UpdateNodeProps`. The bridge replaces
 * the whole list and records an undoable operation so reorder and
 * remove are individually undoable.
 */
function ExtraFillsList({
  nodeId,
  nodeVersion,
  onChange,
  disabled,
}: {
  nodeId: string;
  nodeVersion: number;
  onChange?: (changes: UpdateNodeProps) => void;
  disabled: boolean;
}): JSX.Element | null {
  const [extras, setExtras] = useState<FillStyle[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await window.kcreate.document.nodeExtraFills(nodeId);
        if (!cancelled) {
          setExtras(next ?? []);
        }
      } catch {
        if (!cancelled) {
          setExtras([]);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [nodeId, nodeVersion]);

  if (extras === null) {
    return null;
  }

  const commitList = (next: FillStyle[]): void => {
    setExtras(next);
    onChange?.({ extra_fills: next });
  };
  const append = (): void => {
    commitList([...extras, { kind: "solid", ...RGBA_BLACK }]);
  };
  const removeAt = (idx: number): void => {
    commitList(extras.filter((_, i) => i !== idx));
  };
  const moveUp = (idx: number): void => {
    if (idx <= 0) {
      return;
    }
    const next = [...extras];
    const a = next[idx - 1];
    const b = next[idx];
    if (a === undefined || b === undefined) {
      return;
    }
    next[idx - 1] = b;
    next[idx] = a;
    commitList(next);
  };
  const moveDown = (idx: number): void => {
    if (idx < 0 || idx >= extras.length - 1) {
      return;
    }
    const next = [...extras];
    const a = next[idx];
    const b = next[idx + 1];
    if (a === undefined || b === undefined) {
      return;
    }
    next[idx] = b;
    next[idx + 1] = a;
    commitList(next);
  };
  const replaceAt = (idx: number, value: FillStyle): void => {
    const next = [...extras];
    next[idx] = value;
    commitList(next);
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
      <SectionLabel>Extra fills</SectionLabel>
      {extras.length === 0 ? (
        <Hint>None.</Hint>
      ) : (
        extras.map((entry, idx) => (
          <ExtraFillRow
            key={idx}
            index={idx}
            total={extras.length}
            fill={entry}
            disabled={disabled}
            onChange={(next) => replaceAt(idx, next)}
            onRemove={() => removeAt(idx)}
            onMoveUp={() => moveUp(idx)}
            onMoveDown={() => moveDown(idx)}
          />
        ))
      )}
      <button
        type="button"
        onClick={append}
        disabled={disabled}
        style={{
          fontSize: 11,
          padding: "4px 6px",
          background: colors.bgSoft,
          color: colors.text,
          border: `1px solid ${colors.border}`,
          borderRadius: 4,
          cursor: disabled ? "not-allowed" : "pointer",
          opacity: disabled ? 0.6 : 1,
        }}
      >
        + Add fill
      </button>
    </div>
  );
}

function ExtraFillRow({
  index,
  total,
  fill,
  disabled,
  onChange,
  onRemove,
  onMoveUp,
  onMoveDown,
}: {
  index: number;
  total: number;
  fill: FillStyle;
  disabled: boolean;
  onChange: (next: FillStyle) => void;
  onRemove: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
        padding: spacing.xs,
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: 4,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 4,
        }}
      >
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          {`#${index + 1} / ${total}`} · {fill.kind}
        </span>
        <div style={{ display: "flex", gap: 4 }}>
          <button
            type="button"
            onClick={onMoveUp}
            disabled={disabled || index === 0}
            title="Move up"
            style={extraFillIconButtonStyle(disabled || index === 0)}
          >
            ↑
          </button>
          <button
            type="button"
            onClick={onMoveDown}
            disabled={disabled || index === total - 1}
            title="Move down"
            style={extraFillIconButtonStyle(disabled || index === total - 1)}
          >
            ↓
          </button>
          <button
            type="button"
            onClick={onRemove}
            disabled={disabled}
            title="Remove"
            style={extraFillIconButtonStyle(disabled)}
          >
            ✕
          </button>
        </div>
      </div>
      <FillKindPicker fill={fill} onCommit={onChange} />
      {fill.kind === "solid" ? (
        <SolidFillEditor fill={fill} onCommit={onChange} />
      ) : null}
    </div>
  );
}

function extraFillIconButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 11,
    width: 22,
    height: 22,
    padding: 0,
    background: "transparent",
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: 4,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
  };
}

/// Header label for a section inside PropertiesPanel.
function SectionLabel({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <span
      style={{
        fontSize: 11,
        fontWeight: 600,
        color: colors.textMuted,
        textTransform: "uppercase",
        letterSpacing: "0.04em",
      }}
    >
      {children}
    </span>
  );
}

/// Three-way kind picker: None / Solid / Gradient. Switching to
/// solid / gradient seeds a reasonable default — pure black for
/// solid, a two-stop white → black linear gradient for gradient —
/// so the canvas reflects the user's intent immediately rather
/// than rendering nothing until they pick a colour.
function FillKindPicker({
  fill,
  onCommit,
}: {
  fill: FillStyle;
  onCommit: (next: FillStyle) => void;
}): JSX.Element {
  const options: Array<{ kind: FillStyle["kind"]; label: string }> = [
    { kind: "none", label: "None" },
    { kind: "solid", label: "Solid" },
    { kind: "gradient", label: "Gradient" },
  ];
  return (
    <div style={{ display: "flex", gap: 4 }}>
      {options.map((opt) => {
        const active = fill.kind === opt.kind;
        return (
          <button
            key={opt.kind}
            type="button"
            onClick={() => {
              if (active) {
                return;
              }
              onCommit(seedFillForKind(opt.kind, fill));
            }}
            style={{
              flex: 1,
              padding: "4px 6px",
              fontSize: 11,
              fontWeight: 500,
              background: active ? colors.accent : colors.bgSoft,
              color: active ? colors.textInverse : colors.text,
              border: `1px solid ${active ? colors.accent : colors.border}`,
              borderRadius: 4,
              cursor: "pointer",
            }}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

/// Build a sensible default `FillStyle` for a newly-picked kind.
/// We preserve the existing solid colour when switching solid →
/// gradient (using it as the gradient's first stop) and the first
/// gradient stop's colour when switching gradient → solid so the
/// transition isn't jarring.
function seedFillForKind(
  kind: FillStyle["kind"],
  current: FillStyle,
): FillStyle {
  switch (kind) {
    case "none":
      return { kind: "none" };
    case "solid": {
      const c = pickColorFromFill(current) ?? RGBA_BLACK;
      return { kind: "solid", ...c };
    }
    case "gradient": {
      const c = pickColorFromFill(current) ?? RGBA_BLACK;
      return {
        kind: "gradient",
        shape: "linear",
        from: { x: 0, y: 0 },
        to: { x: 1, y: 0 },
        stops: [
          { offset: 0, color: c },
          { offset: 1, color: RGBA_WHITE },
        ],
      };
    }
  }
}

function pickColorFromFill(fill: FillStyle): RgbaColor | null {
  if (fill.kind === "solid") {
    return { r: fill.r, g: fill.g, b: fill.b, a: fill.a };
  }
  if (fill.kind === "gradient") {
    const first = fill.stops[0];
    if (first !== undefined) {
      return first.color;
    }
  }
  return null;
}

const RGBA_BLACK: RgbaColor = { r: 0, g: 0, b: 0, a: 1 };
const RGBA_WHITE: RgbaColor = { r: 1, g: 1, b: 1, a: 1 };

/// Solid fill editor: HTML5 colour picker (RGB) + alpha slider
/// (HTML5 doesn't have a native alpha-aware picker; the OS pickers
/// throw away alpha when round-tripped through `<input type="color">`).
function SolidFillEditor({
  fill,
  onCommit,
}: {
  fill: Extract<FillStyle, { kind: "solid" }>;
  onCommit: (next: FillStyle) => void;
}): JSX.Element {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <Row>
        <Field label="Colour">
          <input
            type="color"
            value={rgbaToHex(fill)}
            onChange={(e) => {
              const c = hexToRgba(e.target.value, fill.a);
              onCommit({ kind: "solid", ...c });
            }}
            style={{
              ...textInputStyle,
              padding: 0,
              height: 28,
              cursor: "pointer",
            }}
          />
        </Field>
        <Field label={`Alpha (${(fill.a * 100).toFixed(0)}%)`}>
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={fill.a}
            onChange={(e) => {
              const a = Number.parseFloat(e.target.value);
              onCommit({ kind: "solid", r: fill.r, g: fill.g, b: fill.b, a });
            }}
          />
        </Field>
      </Row>
    </div>
  );
}

/// Gradient editor: shape picker, stop list, add / remove buttons.
///
/// The `disabled` prop is honoured here even though the parent
/// `<fieldset disabled>` already cascades to every native `<input>`
/// / `<button>`. We keep the explicit gate for two reasons:
///
/// 1. The stop offset slider is a continuous input that could be
///    fired programmatically (e.g. from a future "snap to even
///    offsets" affordance) without going through a click. The
///    explicit `disabled` here protects the commit path from a
///    non-user-driven mutation reaching the bridge.
/// 2. The "Add stop" / "Remove stop" buttons render with a
///    distinct affordance (greyed-out + cursor: not-allowed) so
///    a remote peer holding the lock is unambiguous; the
///    fieldset-disabled cursor change alone is subtle.
function GradientFillEditor({
  fill,
  onCommit,
  disabled,
}: {
  fill: Extract<FillStyle, { kind: "gradient" }>;
  onCommit: (next: FillStyle) => void;
  disabled: boolean;
}): JSX.Element {
  const shape: "linear" | "radial" = fill.shape;
  const setShape = (next: "linear" | "radial"): void => {
    if (next === shape) {
      return;
    }
    // Switching shape preserves the stops but resets the
    // geometry to a sensible default for the new shape — a
    // diagonal linear sweep or a centred radial blob — because
    // converting the existing geometry between the two
    // parameterisations doesn't have an obvious right answer.
    if (next === "linear") {
      onCommit({
        kind: "gradient",
        shape: "linear",
        from: { x: 0, y: 0 },
        to: { x: 1, y: 0 },
        stops: fill.stops,
      });
    } else {
      onCommit({
        kind: "gradient",
        shape: "radial",
        center: { x: 0.5, y: 0.5 },
        radius: 0.5,
        stops: fill.stops,
      });
    }
  };

  const setStops = (stops: GradientStop[]): void => {
    const sorted = [...stops].sort((a, b) => a.offset - b.offset);
    if (fill.shape === "linear") {
      onCommit({
        kind: "gradient",
        shape: "linear",
        from: fill.from,
        to: fill.to,
        stops: sorted,
      });
    } else {
      onCommit({
        kind: "gradient",
        shape: "radial",
        center: fill.center,
        radius: fill.radius,
        stops: sorted,
      });
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
      {/*
       * Honest UX: the scene-sync layer's `node_fill` in
       * `crates/kcreate_bridge/src/scene_sync.rs` returns `None`
       * for `FillStyle::Gradient` today, so a vector layer with a
       * gradient fill renders as invisible on the canvas (we
       * intentionally don't fall back to "solid first stop" because
       * that would silently misrepresent the saved gradient).
       * Authoring still works — the gradient persists in the
       * document graph and on disk — but users would otherwise see
       * the shape disappear when they pick a gradient. Surface
       * that explicitly here rather than leaving them to discover
       * it by surprise. Will be removed once the renderer's
       * gradient-expander path lands.
       */}
      <div style={gradientRenderNoticeStyle}>
        Gradient fills are saved correctly but not yet painted on the
        canvas. The shape will appear invisible here until the
        gradient renderer lands; export and persistence are
        unaffected.
      </div>
      <Row>
        <button
          type="button"
          onClick={() => setShape("linear")}
          disabled={disabled}
          style={{
            flex: 1,
            padding: "4px 6px",
            fontSize: 11,
            background: shape === "linear" ? colors.accent : colors.bgSoft,
            color: shape === "linear" ? colors.textInverse : colors.text,
            border: `1px solid ${shape === "linear" ? colors.accent : colors.border}`,
            borderRadius: 4,
            cursor: disabled ? "not-allowed" : "pointer",
          }}
        >
          Linear
        </button>
        <button
          type="button"
          onClick={() => setShape("radial")}
          disabled={disabled}
          style={{
            flex: 1,
            padding: "4px 6px",
            fontSize: 11,
            background: shape === "radial" ? colors.accent : colors.bgSoft,
            color: shape === "radial" ? colors.textInverse : colors.text,
            border: `1px solid ${shape === "radial" ? colors.accent : colors.border}`,
            borderRadius: 4,
            cursor: disabled ? "not-allowed" : "pointer",
          }}
        >
          Radial
        </button>
      </Row>
      {fill.shape === "linear" ? (
        <LinearGradientEndpoints
          from={fill.from}
          to={fill.to}
          onCommit={(from, to) =>
            onCommit({
              kind: "gradient",
              shape: "linear",
              from,
              to,
              stops: fill.stops,
            })
          }
        />
      ) : (
        <RadialGradientGeometry
          center={fill.center}
          gradientRadius={fill.radius}
          onCommit={(center, radius) =>
            onCommit({
              kind: "gradient",
              shape: "radial",
              center,
              radius,
              stops: fill.stops,
            })
          }
        />
      )}
      <SectionLabel>Stops</SectionLabel>
      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        {fill.stops.map((stop, idx) => (
          <GradientStopEditor
            // Stops are identified positionally; sorting/insertion
            // shuffles the array, and offsets aren't unique (two
            // stops can share an offset). Using the index as the
            // key is fine because the component body is stateless
            // — the only state lives on the controlled inputs
            // which are re-driven by props on every render.
            key={idx}
            stop={stop}
            disabled={disabled}
            onCommit={(next) => {
              const stops = fill.stops.slice();
              stops[idx] = next;
              setStops(stops);
            }}
            onRemove={
              fill.stops.length > 2
                ? () => {
                    const stops = fill.stops.slice();
                    stops.splice(idx, 1);
                    setStops(stops);
                  }
                : null
            }
          />
        ))}
      </div>
      <button
        type="button"
        onClick={() => {
          // New stop is inserted at the midpoint of the LARGEST
          // current gap between adjacent stops. This matches the
          // standard pattern across design tools (Figma, Adobe XD,
          // Sketch) and avoids the Zeno's-paradox clustering toward
          // offset 1.0 that "midpoint between last two stops"
          // produces on repeated clicks. Colour linearly interpolates
          // between the gap's endpoints so the new stop sits on the
          // existing ramp rather than introducing a discontinuity.
          //
          // Edge cases:
          // - Empty / single-stop input shouldn't reach here (we
          //   always seed two stops), but if it does, fall back to
          //   offset 0.5 + black.
          // - Already-sorted input is guaranteed by `setStops`
          //   sorting on commit, so we can walk adjacent pairs.
          let widestStart = fill.stops[0];
          let widestEnd = fill.stops[fill.stops.length - 1];
          let widestGap = -1;
          for (let i = 0; i < fill.stops.length - 1; i++) {
            const a = fill.stops[i];
            const b = fill.stops[i + 1];
            if (a === undefined || b === undefined) {
              continue;
            }
            const gap = b.offset - a.offset;
            if (gap > widestGap) {
              widestGap = gap;
              widestStart = a;
              widestEnd = b;
            }
          }
          const offset =
            widestStart !== undefined && widestEnd !== undefined
              ? (widestStart.offset + widestEnd.offset) / 2
              : 0.5;
          const color =
            widestStart !== undefined && widestEnd !== undefined
              ? {
                  r: (widestStart.color.r + widestEnd.color.r) / 2,
                  g: (widestStart.color.g + widestEnd.color.g) / 2,
                  b: (widestStart.color.b + widestEnd.color.b) / 2,
                  a: (widestStart.color.a + widestEnd.color.a) / 2,
                }
              : RGBA_BLACK;
          setStops([...fill.stops, { offset, color }]);
        }}
        disabled={disabled}
        style={{
          ...buttonStyle,
          opacity: disabled ? 0.55 : 1,
          cursor: disabled ? "not-allowed" : "pointer",
        }}
      >
        Add stop
      </button>
    </div>
  );
}

/// Single gradient stop row: offset slider + colour picker +
/// alpha slider + remove button. Disabled gate sits on every
/// control individually for defence-in-depth (see the gate doc
/// on `GradientFillEditor`).
function GradientStopEditor({
  stop,
  disabled,
  onCommit,
  onRemove,
}: {
  stop: GradientStop;
  disabled: boolean;
  onCommit: (next: GradientStop) => void;
  /// `null` when the stop cannot be removed — we enforce a
  /// minimum of two stops so the gradient is never degenerate.
  /// Passing a callback unconditionally and disabling the button
  /// at the wrong moment would surface a clickable-but-no-op
  /// button to the user; gating at the prop level is cleaner.
  onRemove: (() => void) | null;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        gap: 6,
        alignItems: "center",
        padding: "4px",
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: 4,
      }}
    >
      <input
        type="color"
        value={rgbaToHex(stop.color)}
        onChange={(e) =>
          onCommit({
            offset: stop.offset,
            color: hexToRgba(e.target.value, stop.color.a),
          })
        }
        disabled={disabled}
        style={{
          width: 28,
          height: 24,
          border: "none",
          background: "transparent",
          padding: 0,
          cursor: disabled ? "not-allowed" : "pointer",
        }}
      />
      <input
        type="range"
        min={0}
        max={1}
        step={0.001}
        value={stop.offset}
        onChange={(e) =>
          onCommit({
            offset: Number.parseFloat(e.target.value),
            color: stop.color,
          })
        }
        disabled={disabled}
        style={{ flex: 1, cursor: disabled ? "not-allowed" : "pointer" }}
        aria-label="Stop offset"
      />
      <span
        style={{
          fontSize: 10,
          color: colors.textMuted,
          minWidth: 32,
          textAlign: "right",
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {(stop.offset * 100).toFixed(0)}%
      </span>
      <button
        type="button"
        onClick={() => onRemove?.()}
        disabled={disabled || onRemove === null}
        style={{
          padding: "2px 6px",
          fontSize: 11,
          background: "transparent",
          color: colors.textMuted,
          border: `1px solid ${colors.border}`,
          borderRadius: 4,
          cursor:
            disabled || onRemove === null ? "not-allowed" : "pointer",
        }}
        aria-label="Remove stop"
        title={onRemove === null ? "A gradient needs at least two stops" : "Remove stop"}
      >
        ×
      </button>
    </div>
  );
}

/// Two-point linear gradient geometry editor. Coordinates are in
/// the node-relative `[0, 1]` square that the renderer's
/// `node_fill` will map back to the node bounds.
function LinearGradientEndpoints({
  from,
  to,
  onCommit,
}: {
  from: Point2D;
  to: Point2D;
  onCommit: (from: Point2D, to: Point2D) => void;
}): JSX.Element {
  return (
    <Row>
      <Field label="From">
        <Row>
          <NumberStub
            value={from.x}
            onChange={(v) => onCommit({ x: v, y: from.y }, to)}
            ariaLabel="From X"
          />
          <NumberStub
            value={from.y}
            onChange={(v) => onCommit({ x: from.x, y: v }, to)}
            ariaLabel="From Y"
          />
        </Row>
      </Field>
      <Field label="To">
        <Row>
          <NumberStub
            value={to.x}
            onChange={(v) => onCommit(from, { x: v, y: to.y })}
            ariaLabel="To X"
          />
          <NumberStub
            value={to.y}
            onChange={(v) => onCommit(from, { x: to.x, y: v })}
            ariaLabel="To Y"
          />
        </Row>
      </Field>
    </Row>
  );
}

/// Centre + radius radial gradient geometry editor. Like
/// `LinearGradientEndpoints`, coordinates are normalised to the
/// node bounds.
///
/// Prop is named `gradientRadius` rather than `radius` to avoid
/// shadowing the file-level `radius` styling token imported from
/// `../styles/tokens` (used elsewhere in this file for
/// `borderRadius: radius.pill` etc). The component body doesn't
/// reach for the token, but the rename keeps future editors from
/// being surprised when they try to use it inside this function.
function RadialGradientGeometry({
  center,
  gradientRadius,
  onCommit,
}: {
  center: Point2D;
  gradientRadius: number;
  onCommit: (center: Point2D, radius: number) => void;
}): JSX.Element {
  return (
    <Row>
      <Field label="Centre">
        <Row>
          <NumberStub
            value={center.x}
            onChange={(v) => onCommit({ x: v, y: center.y }, gradientRadius)}
            ariaLabel="Centre X"
          />
          <NumberStub
            value={center.y}
            onChange={(v) => onCommit({ x: center.x, y: v }, gradientRadius)}
            ariaLabel="Centre Y"
          />
        </Row>
      </Field>
      <Field label="Radius">
        <NumberStub
          value={gradientRadius}
          onChange={(v) => onCommit(center, v)}
          ariaLabel="Radius"
        />
      </Field>
    </Row>
  );
}

/// Compact number input for gradient geometry. Non-finite inputs
/// (Infinity / NaN) are ignored because they don't round-trip
/// through JSON — the editor cannot author a value that the
/// bridge can't persist (see the f64 sentinel knowledge note).
function NumberStub({
  value,
  onChange,
  ariaLabel,
}: {
  value: number;
  onChange: (v: number) => void;
  ariaLabel: string;
}): JSX.Element {
  return (
    <input
      type="number"
      step={0.01}
      value={value}
      aria-label={ariaLabel}
      onChange={(e) => {
        const next = Number.parseFloat(e.target.value);
        if (Number.isFinite(next)) {
          onChange(next);
        }
      }}
      style={{ ...textInputStyle, width: "100%" }}
    />
  );
}

/// HTML5 colour picker speaks `#rrggbb`. RGBA in our `FillStyle`
/// is floats in `[0, 1]` — clamp before quantising so an
/// out-of-range value (shouldn't happen from the bridge, but
/// defends against future code paths) produces a valid hex.
function rgbaToHex(c: RgbaColor): string {
  const r = clamp01(c.r);
  const g = clamp01(c.g);
  const b = clamp01(c.b);
  const to = (n: number): string =>
    Math.round(n * 255).toString(16).padStart(2, "0");
  return `#${to(r)}${to(g)}${to(b)}`;
}

function hexToRgba(hex: string, a: number): RgbaColor {
  // Accept `#rgb` or `#rrggbb`. Anything else returns black.
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex);
  const v = m?.[1];
  if (v === undefined) {
    return { r: 0, g: 0, b: 0, a };
  }
  if (v.length === 3) {
    const r = v[0] ?? "0";
    const g = v[1] ?? "0";
    const b = v[2] ?? "0";
    return {
      r: Number.parseInt(r + r, 16) / 255,
      g: Number.parseInt(g + g, 16) / 255,
      b: Number.parseInt(b + b, 16) / 255,
      a,
    };
  }
  return {
    r: Number.parseInt(v.slice(0, 2), 16) / 255,
    g: Number.parseInt(v.slice(2, 4), 16) / 255,
    b: Number.parseInt(v.slice(4, 6), 16) / 255,
    a,
  };
}

function clamp01(n: number): number {
  if (!Number.isFinite(n)) {
    return 0;
  }
  return Math.min(1, Math.max(0, n));
}

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
  return <LayoutControlsForFrame node={node} stored={stored} layout={layout} />;
}

/**
 * The actual flex/grid editor body. Pulled into its own component
 * so we can host a real `useState` for the mode toggle. Using
 * `useState` instead of a derived value makes the `SegmentedControl`
 * react synchronously to the user's click instead of waiting for the
 * bridge round-trip + `refreshTree` to repopulate `stored.kind`,
 * which used to leave the active segment visually stuck on the
 * previous mode until the document re-fetched.
 */
function LayoutControlsForFrame({
  node,
  stored,
  layout,
}: {
  node: NodeInfo;
  stored: ParsedLayout | null;
  layout: LayoutHandlers;
}): JSX.Element {
  const storedKind: "flex" | "grid" = stored?.kind ?? "flex";
  const [mode, setMode] = useState<"flex" | "grid">(storedKind);

  // Sync the optimistic mode back to the persisted value when the
  // document refresh lands. This also handles the case where a
  // different surface (e.g. an undo) flips the mode underneath us.
  useEffect(() => {
    setMode(storedKind);
  }, [storedKind]);

  const onModeChange = (next: "flex" | "grid"): void => {
    // Optimistic UI update first so the segmented control flips
    // immediately. The bridge round-trip below catches up the
    // persisted layout config and refreshes the tree.
    setMode(next);
    void (async () => {
      // `setFlex` / `setGrid` persists the layout config in the
      // document; `recompute` then reads it back to compute child
      // bounds. Sequencing them with `await` (rather than firing
      // both concurrently) avoids a one-frame flicker where
      // `recompute` snaps children to the previous mode's geometry.
      if (next === "flex") {
        await layout.setFlex(
          node.id,
          stored?.kind === "flex" ? stored.config : DEFAULT_FLEX,
        );
      } else {
        await layout.setGrid(
          node.id,
          stored?.kind === "grid" ? stored.config : DEFAULT_GRID,
        );
      }
      await layout.recompute(node.id);
    })();
  };
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
            onChange={onModeChange}
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

type InspectTarget = "css" | "tailwind" | "react_style";

const INSPECT_TARGETS: ReadonlyArray<{
  id: InspectTarget;
  label: string;
  language: string;
}> = [
  { id: "css", label: "CSS", language: "css" },
  { id: "tailwind", label: "Tailwind", language: "html" },
  { id: "react_style", label: "React style", language: "tsx" },
];

/**
 * Inspect-mode panel. Fetches the three code-gen snippets (CSS,
 * Tailwind utility list, React inline style) from the bridge for
 * the currently selected node and lets the user copy any of them
 * to the clipboard.
 *
 * The fetch is debounced behind a `useEffect` keyed on the node's
 * `id` and `version` (the version bumps on every mutation in the
 * bridge), so dragging a value slider re-fetches but a transient
 * rerender that doesn't change either does not.
 */
function InspectPanel({ node }: { node: NodeInfo | null }): JSX.Element {
  const [code, setCode] = useState<InspectCode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [target, setTarget] = useState<InspectTarget>("css");
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  // The `node` reference itself changes when the parent reruns a
  // document fetch (refreshTree builds a fresh NodeInfo array), so
  // we re-fetch the inspect output any time `node` is identity-new.
  // For the steady state (no edits) the reference is stable, so we
  // do not refetch on every render.
  useEffect(() => {
    if (!node) {
      setCode(null);
      setError(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const result = await window.kcreate.document.inspectNode(node.id);
        if (!cancelled) {
          setCode(result);
          setError(null);
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    })();
    return (): void => {
      cancelled = true;
    };
  }, [node]);

  if (!node) {
    return <Hint>Select a layer to inspect its computed state.</Hint>;
  }
  if (error) {
    return <Hint>Inspect failed: {error}</Hint>;
  }
  if (!code) {
    return <Hint>Loading inspect output…</Hint>;
  }
  const body = code[target];
  const language = INSPECT_TARGETS.find((t) => t.id === target)?.language;
  const onCopy = (): void => {
    void (async () => {
      try {
        await navigator.clipboard.writeText(body);
        setCopyStatus("Copied!");
        window.setTimeout(() => setCopyStatus(null), 1200);
      } catch (e) {
        setCopyStatus(`Copy failed: ${e instanceof Error ? e.message : e}`);
        window.setTimeout(() => setCopyStatus(null), 1800);
      }
    })();
  };
  return (
    <div
      style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}
    >
      <SegmentedControl
        value={target}
        onChange={setTarget}
        options={INSPECT_TARGETS.map((t) => ({ value: t.id, label: t.label }))}
      />
      <pre
        data-lang={language}
        style={{
          background: colors.bgSoft,
          padding: spacing.sm,
          margin: 0,
          borderRadius: radius.card / 2,
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
          fontSize: 11,
          lineHeight: 1.5,
          whiteSpace: "pre-wrap",
          wordBreak: "break-all",
          color: colors.text,
          maxHeight: 360,
          overflow: "auto",
        }}
      >
        {body}
      </pre>
      <div
        style={{ display: "flex", gap: spacing.sm, alignItems: "center" }}
      >
        <button
          type="button"
          onClick={onCopy}
          style={{
            background: colors.accent,
            color: "#fff",
            border: "none",
            borderRadius: radius.card / 2,
            padding: `${spacing.xs}px ${spacing.md}px`,
            fontSize: 12,
            cursor: "pointer",
          }}
        >
          Copy {INSPECT_TARGETS.find((t) => t.id === target)?.label}
        </button>
        {copyStatus ? (
          <span style={{ fontSize: 11, color: colors.textMuted }}>
            {copyStatus}
          </span>
        ) : null}
      </div>
    </div>
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
  disabled = false,
}: {
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        fontSize: 12,
        color: colors.text,
        cursor: disabled ? "not-allowed" : "pointer",
      }}
    >
      <input
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
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

/// Inline notice surfaced inside the gradient editor explaining that
/// gradient fills currently round-trip to disk but don't render on
/// canvas. Styled as a soft warning (yellow accent) rather than an
/// error because authoring is fully functional — only the live
/// preview is missing.
const gradientRenderNoticeStyle: React.CSSProperties = {
  fontSize: 11,
  lineHeight: 1.4,
  padding: "6px 8px",
  background: "rgba(255, 196, 0, 0.12)",
  border: "1px solid rgba(255, 196, 0, 0.45)",
  color: colors.text,
  borderRadius: 4,
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
