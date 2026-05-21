import { useEffect, useState } from "react";

import type { RuntimeStatus } from "../../../shared/scene";
import { colors, font, radius, shadow, spacing } from "../styles/tokens";

/**
 * Job-first create options. The order mirrors PROPOSAL.md §4.1: we
 * lead with the workflows that mainstream non-designers use most.
 */
const CREATE_OPTIONS: ReadonlyArray<{
  id: string;
  title: string;
  blurb: string;
  nodeType: string;
}> = [
  {
    id: "app-ui",
    title: "App / Website UI",
    blurb: "Frames, components, design tokens",
    nodeType: "Page",
  },
  {
    id: "brand",
    title: "Logo / Icon / Brand Kit",
    blurb: "Vector marks, palettes, type",
    nodeType: "Artboard",
  },
  {
    id: "social",
    title: "Social Media Post",
    blurb: "Common sizes for every channel",
    nodeType: "Artboard",
  },
  {
    id: "photo",
    title: "Product Photo Cleanup",
    blurb: "Background removal, retouching",
    nodeType: "RasterLayer",
  },
  {
    id: "deck",
    title: "Pitch Deck / Proposal",
    blurb: "Multi-page layouts, master pages",
    nodeType: "LayoutFrame",
  },
  {
    id: "print",
    title: "Flyer / Poster / Brochure",
    blurb: "Print-ready PDF, CMYK, bleed",
    nodeType: "LayoutFrame",
  },
  {
    id: "dev-export",
    title: "Developer Asset Export",
    blurb: "Icons, SVG, PNG, code snippets",
    nodeType: "VectorLayer",
  },
  {
    id: "import",
    title: "Import Existing File",
    blurb: "SVG, PNG, JPEG, PDF",
    nodeType: "VectorLayer",
  },
];

export interface HomePageProps {
  onOpenEditor: (kind: string) => void;
}

export function HomePage({ onOpenEditor }: HomePageProps): JSX.Element {
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);

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
          <EmptyState>
            No recent projects yet. Create one above — your work is saved
            locally inside <code>.kstudio</code> folders.
          </EmptyState>
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
