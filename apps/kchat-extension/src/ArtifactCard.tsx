// Phase 9 Block A Task 3 — KChat companion artifact preview card.
//
// Renders a rich preview when a conversation message has one of the
// KCreate artifact content types:
//   - kcreate.invite.v1  : project share invite (handled by InviteCard)
//   - kcreate.artifact.v1: a single exported artifact (PNG / SVG /
//                          PDF / WebP / JPEG) shared into the channel.
//
// Clicking "Open in KCreate" dispatches a
// `kcreate://artifact?id=<artifact_id>&project_id=<project_id>`
// deeplink that the KCreate desktop app handles by navigating to
// the project and highlighting the artifact row in the export panel.
import { useCallback, useMemo, useState } from "react";
import { z } from "zod";
import { openDeeplink } from "./host";

/**
 * Canonical content type stamped on KChat messages by the bridge
 * when an artifact is shared. Mirrors `ARTIFACT_CONTENT_TYPE` on
 * the Rust side.
 */
export const KCREATE_ARTIFACT_CONTENT_TYPE = "kcreate.artifact.v1";

/**
 * Wire-format schema for a posted KCreate artifact card. The shape
 * matches `ArtifactCardPayload` in `kcreate_kchat_client/src/protocol.rs`.
 */
export const ArtifactCardPayloadSchema = z.object({
  schemaVersion: z.literal(1),
  artifactId: z.string().min(1),
  projectId: z.string().min(1),
  projectName: z.string().min(1),
  artifactName: z.string().min(1),
  format: z.enum(["png", "jpeg", "svg", "pdf", "webp"]),
  /** Size of the rendered file in bytes (informational). */
  byteSize: z.number().int().nonnegative(),
  /** Optional data-URI thumbnail (base64 PNG, ≤ 256x256). */
  thumbnailDataUri: z.string().optional(),
  /** ISO 8601 UTC timestamp. */
  exportedAt: z.string().min(1),
  /** Optional human-readable description posted by the sender. */
  caption: z.string().optional(),
});
export type ArtifactCardPayload = z.infer<typeof ArtifactCardPayloadSchema>;

interface ArtifactCardProps {
  artifact: ArtifactCardPayload;
  /** Override the deeplink dispatcher (tests). */
  onOpen?: (artifact: ArtifactCardPayload) => Promise<void>;
}

type OpenState =
  | { phase: "idle" }
  | { phase: "opening" }
  | { phase: "error"; message: string };

/**
 * Build the `kcreate://artifact?id=…&project_id=…` deeplink for a
 * given artifact card. The KCreate side parses these parameters,
 * navigates to the project, and highlights the artifact in the
 * Export panel.
 */
export function buildArtifactDeeplink(artifact: ArtifactCardPayload): string {
  const params = new URLSearchParams({
    id: artifact.artifactId,
    project_id: artifact.projectId,
  });
  return `kcreate://artifact?${params.toString()}`;
}

async function defaultOpenArtifact(artifact: ArtifactCardPayload): Promise<void> {
  await openDeeplink(buildArtifactDeeplink(artifact));
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatTimestamp(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) {
    return iso;
  }
  return d.toLocaleString();
}

export function ArtifactCard({
  artifact,
  onOpen,
}: ArtifactCardProps): JSX.Element {
  const [state, setState] = useState<OpenState>({ phase: "idle" });
  const handler = onOpen ?? defaultOpenArtifact;

  const handleOpen = useCallback(async () => {
    setState({ phase: "opening" });
    try {
      await handler(artifact);
      setState({ phase: "idle" });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setState({ phase: "error", message });
    }
  }, [artifact, handler]);

  const formatBadge = useMemo(() => artifact.format.toUpperCase(), [artifact.format]);

  return (
    <article
      data-testid="kcreate-artifact-card"
      data-artifact-id={artifact.artifactId}
      style={cardStyle}
    >
      <header style={headerStyle}>
        <span style={badgeStyle}>{formatBadge}</span>
        <span style={titleStyle}>{artifact.artifactName}</span>
      </header>
      {artifact.thumbnailDataUri !== undefined && (
        <img
          src={artifact.thumbnailDataUri}
          alt={`Thumbnail of ${artifact.artifactName}`}
          style={thumbnailStyle}
        />
      )}
      <dl style={metaStyle}>
        <dt style={metaTermStyle}>Project</dt>
        <dd style={metaValueStyle}>{artifact.projectName}</dd>
        <dt style={metaTermStyle}>Size</dt>
        <dd style={metaValueStyle}>{formatBytes(artifact.byteSize)}</dd>
        <dt style={metaTermStyle}>Exported</dt>
        <dd style={metaValueStyle}>{formatTimestamp(artifact.exportedAt)}</dd>
      </dl>
      {artifact.caption !== undefined && (
        <p style={captionStyle}>{artifact.caption}</p>
      )}
      <button
        type="button"
        onClick={() => void handleOpen()}
        disabled={state.phase === "opening"}
        style={openButtonStyle}
        data-testid="kcreate-artifact-open"
      >
        {state.phase === "opening" ? "Opening…" : "Open in KCreate"}
      </button>
      {state.phase === "error" && (
        <p role="alert" style={errorStyle}>
          {state.message}
        </p>
      )}
    </article>
  );
}

const cardStyle = {
  display: "flex",
  flexDirection: "column" as const,
  gap: 8,
  padding: 12,
  background: "#13161b",
  border: "1px solid #1f2329",
  borderRadius: 8,
  color: "#e7e8ea",
} as const;

const headerStyle = {
  display: "flex",
  alignItems: "center",
  gap: 8,
} as const;

const badgeStyle = {
  background: "#1f4e79",
  color: "white",
  fontSize: 10,
  fontWeight: 700,
  padding: "2px 6px",
  borderRadius: 4,
  letterSpacing: 0.5,
} as const;

const titleStyle = { fontWeight: 600, fontSize: 14 } as const;

const thumbnailStyle = {
  maxWidth: "100%",
  maxHeight: 180,
  objectFit: "contain" as const,
  background: "#0e1013",
  borderRadius: 4,
} as const;

const metaStyle = {
  display: "grid",
  gridTemplateColumns: "max-content 1fr",
  gap: "2px 8px",
  margin: 0,
  fontSize: 12,
} as const;

const metaTermStyle = {
  color: "#9aa0a6",
  margin: 0,
} as const;

const metaValueStyle = {
  margin: 0,
  color: "#e7e8ea",
} as const;

const captionStyle = {
  margin: 0,
  fontSize: 12,
  color: "#c2c7cd",
  fontStyle: "italic" as const,
} as const;

const openButtonStyle = {
  background: "#1f4e79",
  color: "white",
  border: "none",
  borderRadius: 4,
  padding: "6px 10px",
  fontWeight: 600,
  cursor: "pointer",
  alignSelf: "flex-start" as const,
} as const;

const errorStyle = {
  margin: 0,
  color: "#ff8585",
  fontSize: 12,
} as const;
