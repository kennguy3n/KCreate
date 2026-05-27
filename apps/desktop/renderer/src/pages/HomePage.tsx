import { useEffect, useState } from "react";

import type {
  RecentProjectInfo,
  RuntimeStatus,
  ThumbnailBytes,
} from "../../../shared/scene";
import { colors, font, radius, shadow, spacing } from "../styles/tokens";

/**
 * Job-first create options. The order mirrors PROPOSAL.md §4.1: we
 * lead with the workflows that mainstream non-designers use most.
 *
 * `defaultArtboard` is wired through to the editor on click so the
 * new project lands on a canvas sized for the intended workflow
 * (Desktop UI, Instagram post, A4 print, …). The actual artboard is
 * created in Rust via `window.kcreate.artboard.create()` once the
 * project exists.
 */
export interface CreateOption {
  id: string;
  title: string;
  blurb: string;
  nodeType: string;
  /// Artboard to create automatically when this card is clicked.
  /// `null` means the user picks dimensions later (e.g. the Import
  /// flow doesn't create a fresh artboard).
  defaultArtboard: { name: string; width: number; height: number } | null;
}

export const CREATE_OPTIONS: ReadonlyArray<CreateOption> = [
  {
    id: "app-ui",
    title: "App / Website UI",
    blurb: "Frames, components, design tokens",
    nodeType: "Page",
    defaultArtboard: { name: "Desktop", width: 1440, height: 900 },
  },
  {
    id: "brand",
    title: "Logo / Icon / Brand Kit",
    blurb: "Vector marks, palettes, type",
    nodeType: "Artboard",
    defaultArtboard: { name: "Logo", width: 1024, height: 1024 },
  },
  {
    id: "social",
    title: "Social Media Post",
    blurb: "Common sizes for every channel",
    nodeType: "Artboard",
    defaultArtboard: { name: "Instagram Post", width: 1080, height: 1080 },
  },
  {
    id: "photo",
    title: "Product Photo Cleanup",
    blurb: "Background removal, retouching",
    nodeType: "RasterLayer",
    defaultArtboard: { name: "Photo", width: 2048, height: 2048 },
  },
  {
    id: "deck",
    title: "Pitch Deck / Proposal",
    blurb: "Multi-page layouts, master pages",
    nodeType: "LayoutFrame",
    defaultArtboard: { name: "Slide", width: 1920, height: 1080 },
  },
  {
    id: "print",
    title: "Flyer / Poster / Brochure",
    blurb: "Print-ready PDF, CMYK, bleed",
    nodeType: "LayoutFrame",
    // A4 @ 300dpi: 2480 × 3508. Same default as the standard A4 preset
    // in `kcreate_core::node::standard_presets()`.
    defaultArtboard: { name: "A4", width: 2480, height: 3508 },
  },
  {
    id: "dev-export",
    title: "Developer Asset Export",
    blurb: "Icons, SVG, PNG, code snippets",
    nodeType: "VectorLayer",
    defaultArtboard: { name: "Icon", width: 512, height: 512 },
  },
  {
    id: "import",
    title: "Import Existing File",
    blurb: "SVG, PNG, JPEG, PDF",
    nodeType: "VectorLayer",
    defaultArtboard: null,
  },
];

export interface HomePageProps {
  onOpenEditor: (kind: string) => void;
  /**
   * Fired when the user clicks a card on the "Recent projects" grid.
   * The shell wires this to `window.kcreate.document.projectOpen(path)`
   * followed by an editor route push. Receives the absolute
   * `.kstudio` directory path.
   */
  onOpenProject?: (projectDir: string) => void;
}

/**
 * `Idle` — the renderer hasn't been asked yet (initial mount).
 * `Loading` — first request is in flight; show a subtle placeholder.
 * `Ready` — the renderer has the list; render the grid (or empty state).
 * `Error` — the bridge call threw; surface the message inline so the
 * user can report it but don't block the rest of the HomePage.
 */
type RecentsLoadState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; items: RecentProjectInfo[] }
  | { kind: "error"; message: string };

/**
 * `data:image/png;base64,…` is the cheapest way to ship cached PNG
 * bytes from the bridge into a React `<img>` without taking a trip
 * through `URL.createObjectURL` (which leaks across HMR reloads and
 * needs an explicit `revokeObjectURL`). The bridge always emits
 * standard base64 (not URL-safe), so we can splice straight into the
 * `data:` URL.
 */
function dataUrlFor(bytes: ThumbnailBytes): string {
  return `data:${bytes.mime};base64,${bytes.bytesBase64}`;
}

export function HomePage({
  onOpenEditor,
  onOpenProject,
}: HomePageProps): JSX.Element {
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [recents, setRecents] = useState<RecentsLoadState>({ kind: "idle" });
  // Cover-bytes cache keyed by `.kstudio` path. Held outside `recents`
  // so refreshing the roster (e.g. after creating a new project) does
  // not force every `<img>` to re-decode its base64.
  const [covers, setCovers] = useState<
    Record<string, ThumbnailBytes | null>
  >({});

  useEffect(() => {
    let cancelled = false;
    void window.kcreate.runtime
      .status()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          const msg = e instanceof Error ? e.message : String(e);
          setStatusError(msg);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setRecents({ kind: "loading" });
    void window.kcreate.recentProjects
      .list()
      .then((items) => {
        if (cancelled) return;
        setRecents({ kind: "ready", items });
        // Fan out one cover-bytes lookup per project. Each call hits
        // the on-disk cache, so we don't worry about concurrent
        // renderer work — failures are silent and degrade to "no
        // cover" so a single bad project never breaks the whole grid.
        for (const item of items) {
          if (item.cover === null) continue;
          void window.kcreate.recentProjects
            .coverBytes(item.path)
            .then((bytes) => {
              if (cancelled) return;
              setCovers((prev) => ({ ...prev, [item.path]: bytes }));
            })
            .catch(() => {
              if (cancelled) return;
              setCovers((prev) => ({ ...prev, [item.path]: null }));
            });
        }
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        setRecents({ kind: "error", message: msg });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        background: colors.bgSoft,
        fontFamily: font.family,
        color: colors.text,
        overflowY: "auto",
      }}
    >
      <header
        style={{
          padding: `${spacing.lg}px ${spacing.xl}px`,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          background: colors.bg,
          borderBottom: `1px solid ${colors.border}`,
        }}
      >
        <div style={{ fontSize: 20, fontWeight: 600 }}>KCreate</div>
        <RuntimeBadge status={status} error={statusError} />
      </header>

      <main
        style={{
          maxWidth: 1100,
          width: "100%",
          margin: "0 auto",
          padding: `${spacing.xl}px ${spacing.xl}px`,
          display: "flex",
          flexDirection: "column",
          gap: spacing.xl,
        }}
      >
        <Section title="Create new">
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))",
              gap: spacing.md,
            }}
          >
            {CREATE_OPTIONS.map((opt) => (
              <CreateCard
                key={opt.id}
                title={opt.title}
                blurb={opt.blurb}
                onClick={() => onOpenEditor(opt.id)}
              />
            ))}
          </div>
        </Section>

        <Section title="Recent projects">
          <RecentProjectsGrid
            state={recents}
            covers={covers}
            onOpenProject={onOpenProject ?? null}
          />
        </Section>
      </main>
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
    <section style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <h2
        style={{
          margin: 0,
          fontSize: 16,
          fontWeight: 600,
          color: colors.text,
          letterSpacing: 0.2,
        }}
      >
        {title}
      </h2>
      {children}
    </section>
  );
}

function CreateCard({
  title,
  blurb,
  onClick,
}: {
  title: string;
  blurb: string;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        textAlign: "left",
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        padding: spacing.md,
        boxShadow: shadow.card,
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
        transition: "box-shadow 120ms ease, transform 120ms ease",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.boxShadow = shadow.cardHover;
        e.currentTarget.style.transform = "translateY(-1px)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.boxShadow = shadow.card;
        e.currentTarget.style.transform = "translateY(0)";
      }}
    >
      <span style={{ fontSize: 15, fontWeight: 600, color: colors.text }}>
        {title}
      </span>
      <span style={{ fontSize: 13, color: colors.textMuted }}>{blurb}</span>
    </button>
  );
}

function RuntimeBadge({
  status,
  error,
}: {
  status: RuntimeStatus | null;
  error: string | null;
}): JSX.Element {
  if (error) {
    return (
      <span style={{ fontSize: 12, color: "#B91C1C" }}>
        runtime probe failed: {error}
      </span>
    );
  }
  if (!status) {
    return <span style={{ fontSize: 12, color: colors.textMuted }}>…</span>;
  }
  const gpuLabel = status.gpuAvailable
    ? (status.gpuName ?? "GPU")
    : "CPU only";
  return (
    <div
      style={{
        display: "flex",
        gap: spacing.sm,
        alignItems: "center",
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.pill,
        padding: "6px 12px",
        fontSize: 12,
        color: colors.textMuted,
      }}
    >
      <span style={{ fontWeight: 600, color: colors.text }}>
        {status.deviceTier}
      </span>
      <span>·</span>
      <span>{status.platform}</span>
      <span>·</span>
      <span>{gpuLabel}</span>
      <span>·</span>
      <span>{status.totalRamMb} MB</span>
    </div>
  );
}

function EmptyState({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div
      style={{
        padding: spacing.lg,
        background: colors.bg,
        border: `1px dashed ${colors.border}`,
        borderRadius: radius.card,
        color: colors.textMuted,
        fontSize: 13,
        lineHeight: 1.5,
      }}
    >
      {children}
    </div>
  );
}

/**
 * Recent-projects grid. Renders one card per `.kstudio` directory on
 * the persistent roster, displaying the cached cover thumbnail when
 * one is available. Falls back to a tinted placeholder when the
 * project hasn't been opened since the cache was introduced.
 *
 * Cards are buttons (not anchors) because the shell controls
 * navigation — `onOpenProject` is fired with the absolute
 * `.kstudio` path so the parent can route through the bridge.
 */
function RecentProjectsGrid({
  state,
  covers,
  onOpenProject,
}: {
  state: RecentsLoadState;
  covers: Record<string, ThumbnailBytes | null>;
  onOpenProject: ((projectDir: string) => void) | null;
}): JSX.Element {
  if (state.kind === "idle" || state.kind === "loading") {
    return (
      <EmptyState>
        Loading recent projects&hellip;
      </EmptyState>
    );
  }
  if (state.kind === "error") {
    return (
      <EmptyState>
        Could not read the recent-projects list:{" "}
        <code>{state.message}</code>
      </EmptyState>
    );
  }
  if (state.items.length === 0) {
    return (
      <EmptyState>
        No recent projects yet. Create one above — your work is saved
        locally inside <code>.kstudio</code> folders.
      </EmptyState>
    );
  }
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
        gap: spacing.md,
      }}
    >
      {state.items.map((item) => (
        <RecentProjectCard
          key={item.path}
          info={item}
          cover={covers[item.path] ?? null}
          onClick={
            onOpenProject ? () => onOpenProject(item.path) : undefined
          }
        />
      ))}
    </div>
  );
}

function RecentProjectCard({
  info,
  cover,
  onClick,
}: {
  info: RecentProjectInfo;
  cover: ThumbnailBytes | null;
  onClick: (() => void) | undefined;
}): JSX.Element {
  const subtitle = formatRelativeIso(info.lastOpenedAt);
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={!onClick}
      title={info.path}
      style={{
        textAlign: "left",
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        padding: 0,
        boxShadow: shadow.card,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        cursor: onClick ? "pointer" : "default",
        transition: "box-shadow 120ms ease, transform 120ms ease",
      }}
      onMouseEnter={(e) => {
        if (!onClick) return;
        e.currentTarget.style.boxShadow = shadow.cardHover;
        e.currentTarget.style.transform = "translateY(-1px)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.boxShadow = shadow.card;
        e.currentTarget.style.transform = "translateY(0)";
      }}
    >
      <div
        style={{
          aspectRatio: "16 / 10",
          background: cover ? colors.bgSoft : colors.border,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          overflow: "hidden",
        }}
      >
        {cover ? (
          <img
            src={dataUrlFor(cover)}
            alt={`${info.name} cover thumbnail`}
            width={cover.width}
            height={cover.height}
            loading="lazy"
            decoding="async"
            style={{
              width: "100%",
              height: "100%",
              objectFit: "contain",
              display: "block",
            }}
          />
        ) : (
          <span style={{ color: colors.textMuted, fontSize: 12 }}>
            no preview
          </span>
        )}
      </div>
      <div
        style={{
          padding: spacing.sm,
          display: "flex",
          flexDirection: "column",
          gap: 2,
        }}
      >
        <span
          style={{
            fontSize: 14,
            fontWeight: 600,
            color: colors.text,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {info.name}
        </span>
        <span style={{ fontSize: 12, color: colors.textMuted }}>
          {subtitle}
        </span>
      </div>
    </button>
  );
}

/**
 * Human-friendly "2h ago" / "yesterday" / "Mar 14" rendering. Falls
 * back to the raw ISO string when parsing fails — the bridge already
 * guarantees RFC 3339 UTC, but defending against drift is cheap.
 */
function formatRelativeIso(iso: string): string {
  const ts = Date.parse(iso);
  if (Number.isNaN(ts)) return iso;
  const deltaMs = Date.now() - ts;
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;
  // `Math.floor` (not `Math.round`) so each bucket stays strictly
  // within its label range: 59m → "59m ago", 60m → "1h ago" (we've
  // already advanced into the next bucket via `deltaMs < hour`),
  // never the visually-wrong "60m ago". Same reasoning for the hour
  // → day and day → week boundaries.
  if (deltaMs < minute) return "just now";
  if (deltaMs < hour) {
    const m = Math.floor(deltaMs / minute);
    return `${m}m ago`;
  }
  if (deltaMs < day) {
    const h = Math.floor(deltaMs / hour);
    return `${h}h ago`;
  }
  if (deltaMs < 7 * day) {
    const d = Math.floor(deltaMs / day);
    return d <= 1 ? "yesterday" : `${d}d ago`;
  }
  return new Date(ts).toLocaleDateString();
}
