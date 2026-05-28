// ArtifactPublishPanel — Phase 8 Block A Task 3.
//
// Publish a rendered artifact (PNG / SVG / PDF / WebP / JPEG) — or
// the active brand kit — into a KChat conversation, and inspect
// previously-published artifacts for that conversation.
//
// Wires `window.kcreate.kchatBackend.publishArtifact`,
// `publishBrandKit`, and `listArtifacts`. The renderer is responsible
// for collecting the bytes (via `export.*`), but Phase 8 added a
// fast path: pass a `KChatArtifactRequest` with a discriminated
// `kind` to `kchatBackend.publishArtifact` and the bridge runs the
// export → multipart-upload flow internally. We use that fast path
// here rather than re-encoding bytes through the renderer; the
// renderer's `export.*` calls only fire for the standalone
// "Download as file" gestures elsewhere in the app.
//
// UX model:
//   * Top: capability probe (`available()`) drives a "KChat backend
//     not linked" empty state. Sign-in / community selection happen
//     elsewhere — this panel assumes a backend is connected and a
//     community has been selected.
//   * Community + conversation selectors (cascading dropdowns).
//   * Format radio (PNG / SVG / PDF / WebP / JPEG / brand kit).
//   * Format-specific options (width / height / scale / quality /
//     background) collapse based on the format choice.
//   * Brand-kit upload path swaps the format-options column for a
//     brand-kit picker.
//   * "Publish" button — runs `publishArtifact` / `publishBrandKit`
//     and surfaces the new artifact at the top of the list.
//   * List: shows previously-published artifacts for the selected
//     conversation, newest first, with thumbnail + metadata.
//
// Failure handling:
//   * Backend errors (auth, rate-limit, payload-too-large) surface
//     in the bottom toast region with a "Retry" affordance.
//   * Bridge transient (e.g. backend unreachable mid-poll) is
//     non-destructive — the panel retains its last good state.

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import type {
  BrandKit,
  KChatArtifactKind,
  KChatArtifactPublishRequest,
  KChatArtifactPublishResult,
  KChatBrandKitArtifactRequest,
  KChatCommunity,
  KChatConversation,
  KChatPublishedArtifact,
  PngExportOptions,
  SvgExportOptions,
  PdfExportOptions,
  WebpExportOptions,
  JpegExportOptions,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ArtifactPublishPanelProps {
  /** Status sink. Same convention as PreflightPanel. */
  onStatus?: (msg: string | null) => void;
}

type FormatChoice =
  | "png"
  | "svg"
  | "pdf"
  | "webp"
  | "jpeg"
  | "brandKit";

interface PngState {
  width: number;
  height: number;
  scale: number;
}
interface SvgState {
  width: number;
  height: number;
  optimize: boolean;
}
interface PdfState {
  widthMm: number;
  heightMm: number;
  title: string;
}
interface WebpState {
  width: number;
  height: number;
  scale: number;
  quality: number;
  lossless: boolean;
}
interface JpegState {
  width: number;
  height: number;
  scale: number;
  quality: number;
}

const PNG_DEFAULTS: PngState = { width: 1920, height: 1080, scale: 1 };
const SVG_DEFAULTS: SvgState = {
  width: 1920,
  height: 1080,
  optimize: true,
};
const PDF_DEFAULTS: PdfState = {
  widthMm: 210,
  heightMm: 297,
  title: "KCreate export",
};
const WEBP_DEFAULTS: WebpState = {
  width: 1920,
  height: 1080,
  scale: 1,
  quality: 85,
  lossless: false,
};
const JPEG_DEFAULTS: JpegState = {
  width: 1920,
  height: 1080,
  scale: 1,
  quality: 90,
};

export function ArtifactPublishPanel({
  onStatus,
}: ArtifactPublishPanelProps): JSX.Element {
  const [available, setAvailable] = useState<boolean | null>(null);
  const [communities, setCommunities] = useState<KChatCommunity[]>([]);
  const [communityId, setCommunityId] = useState<string | null>(null);
  const [conversations, setConversations] = useState<KChatConversation[]>([]);
  const [conversationId, setConversationId] = useState<string | null>(null);
  const [artifacts, setArtifacts] = useState<KChatPublishedArtifact[]>([]);
  const [format, setFormat] = useState<FormatChoice>("png");
  const [preset, setPreset] = useState("");
  const [artboardName, setArtboardName] = useState("");
  const [brandKits, setBrandKits] = useState<BrandKit[]>([]);
  const [brandKitId, setBrandKitId] = useState<string | null>(null);
  const [png, setPng] = useState<PngState>(PNG_DEFAULTS);
  const [svg, setSvg] = useState<SvgState>(SVG_DEFAULTS);
  const [pdf, setPdf] = useState<PdfState>(PDF_DEFAULTS);
  const [webp, setWebp] = useState<WebpState>(WEBP_DEFAULTS);
  const [jpeg, setJpeg] = useState<JpegState>(JPEG_DEFAULTS);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Capability probe + initial community list. Both fire once on
  // mount; we don't poll after that — the backend doesn't surface
  // new communities mid-session without explicit user action
  // (sign-in / refresh) so polling would be wasted bandwidth.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const ok = await window.kcreate.kchatBackend.available();
        if (cancelled) return;
        setAvailable(ok);
        if (!ok) return;
        const list = await window.kcreate.kchatBackend.listCommunities();
        if (cancelled) return;
        setCommunities(list);
        if (list.length > 0 && list[0]) setCommunityId(list[0].id);
      } catch (e) {
        if (!cancelled) {
          setAvailable(false);
          setError(`Backend unavailable: ${errMsg(e)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Load conversations whenever the community changes.
  useEffect(() => {
    if (communityId == null) {
      setConversations([]);
      setConversationId(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const list = await window.kcreate.kchatBackend.listConversations(
          communityId,
        );
        if (cancelled) return;
        setConversations(list);
        setConversationId((prev) => {
          if (prev != null && list.some((c) => c.id === prev)) return prev;
          return list[0]?.id ?? null;
        });
      } catch (e) {
        if (!cancelled) setError(`List conversations failed: ${errMsg(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [communityId]);

  // Load brand kits once (used by the "brand kit" upload format).
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const kits = await window.kcreate.brandKit.list();
        if (cancelled) return;
        setBrandKits(kits);
        if (kits.length > 0 && kits[0]) setBrandKitId(kits[0].id);
      } catch {
        // brandKit.list is not load-bearing for the non-brandKit
        // formats; ignore failure here. The brandKit format will
        // surface an empty picker, which is the correct UX.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Reload the artifact list whenever the conversation changes.
  // Exported for re-use after a publish below.
  const reloadArtifacts = useCallback(
    async (cid: string | null): Promise<void> => {
      if (cid == null) {
        setArtifacts([]);
        return;
      }
      try {
        const list = await window.kcreate.kchatBackend.listArtifacts(cid);
        // Sort newest first by publishedAt. The bridge does not
        // guarantee order — sorting here keeps the panel's
        // contract explicit.
        list.sort((a, b) => b.publishedAt.localeCompare(a.publishedAt));
        setArtifacts(list);
      } catch (e) {
        setError(`List artifacts failed: ${errMsg(e)}`);
      }
    },
    [],
  );

  useEffect(() => {
    void reloadArtifacts(conversationId);
  }, [conversationId, reloadArtifacts]);

  const handlePublish = useCallback(async (): Promise<void> => {
    if (conversationId == null) {
      setError("Pick a conversation first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      let result: KChatArtifactPublishResult;
      if (format === "brandKit") {
        if (brandKitId == null) {
          setError("Pick a brand kit to upload.");
          setBusy(false);
          return;
        }
        const req: KChatBrandKitArtifactRequest = {
          brandKitId,
          ...(preset.trim().length > 0 ? { exportPreset: preset.trim() } : {}),
        };
        result = await window.kcreate.kchatBackend.publishBrandKit(
          conversationId,
          req,
        );
      } else {
        const req = buildArtifactRequest(format, {
          png,
          svg,
          pdf,
          webp,
          jpeg,
        });
        const publishRequest: KChatArtifactPublishRequest = {
          kind: req,
          ...(preset.trim().length > 0 ? { exportPreset: preset.trim() } : {}),
          ...(artboardName.trim().length > 0
            ? { artboardName: artboardName.trim() }
            : {}),
        };
        result = await window.kcreate.kchatBackend.publishArtifact(
          conversationId,
          publishRequest,
        );
      }
      onStatus?.(`Published ${result.kind} artifact ${result.artifactId.slice(0, 8)}.`);
      await reloadArtifacts(conversationId);
    } catch (e) {
      setError(`Publish failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [
    artboardName,
    brandKitId,
    conversationId,
    format,
    jpeg,
    onStatus,
    pdf,
    png,
    preset,
    reloadArtifacts,
    svg,
    webp,
  ]);

  const activeConversation = useMemo(
    () => conversations.find((c) => c.id === conversationId) ?? null,
    [conversationId, conversations],
  );

  if (available === false) {
    return (
      <div
        style={{
          padding: spacing.md,
          fontSize: 12,
          color: colors.textMuted,
        }}
      >
        KChat backend is not linked in this build, so artifact
        publishing is unavailable. Use the file-export panel to save
        a local copy and paste it into KChat manually.
      </div>
    );
  }
  if (available == null) {
    return (
      <div
        style={{
          padding: spacing.md,
          fontSize: 12,
          color: colors.textMuted,
        }}
      >
        Checking KChat backend availability…
      </div>
    );
  }

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
          Publish to KChat
        </h3>
        <small style={{ color: colors.textMuted }}>
          Render the active scene as an artifact and post it to a
          KChat conversation as a rich card.
        </small>
      </header>

      <label style={fieldLabelStyle}>
        Community
        <select
          value={communityId ?? ""}
          onChange={(e) => setCommunityId(e.target.value || null)}
          style={selectStyle}
          disabled={communities.length === 0}
        >
          {communities.length === 0 ? (
            <option value="">No communities</option>
          ) : null}
          {communities.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </label>

      <label style={fieldLabelStyle}>
        Conversation
        <select
          value={conversationId ?? ""}
          onChange={(e) => setConversationId(e.target.value || null)}
          style={selectStyle}
          disabled={conversations.length === 0}
        >
          {conversations.length === 0 ? (
            <option value="">No conversations in this community</option>
          ) : null}
          {conversations.map((c) => (
            <option key={c.id} value={c.id}>
              #{c.name}
            </option>
          ))}
        </select>
      </label>

      <fieldset style={fieldsetStyle}>
        <legend style={legendStyle}>Format</legend>
        <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
          {(["png", "svg", "pdf", "webp", "jpeg", "brandKit"] as const).map(
            (f) => (
              <button
                key={f}
                type="button"
                onClick={() => setFormat(f)}
                style={chipButtonStyle(format === f)}
              >
                {f === "brandKit" ? "Brand kit" : f.toUpperCase()}
              </button>
            ),
          )}
        </div>
      </fieldset>

      {format !== "brandKit" ? (
        <fieldset style={fieldsetStyle}>
          <legend style={legendStyle}>Options</legend>
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {format === "png" ? (
              <PngControls value={png} onChange={setPng} />
            ) : null}
            {format === "svg" ? (
              <SvgControls value={svg} onChange={setSvg} />
            ) : null}
            {format === "pdf" ? (
              <PdfControls value={pdf} onChange={setPdf} />
            ) : null}
            {format === "webp" ? (
              <WebpControls value={webp} onChange={setWebp} />
            ) : null}
            {format === "jpeg" ? (
              <JpegControls value={jpeg} onChange={setJpeg} />
            ) : null}
          </div>
        </fieldset>
      ) : (
        <fieldset style={fieldsetStyle}>
          <legend style={legendStyle}>Brand kit</legend>
          <select
            value={brandKitId ?? ""}
            onChange={(e) => setBrandKitId(e.target.value || null)}
            style={selectStyle}
            disabled={brandKits.length === 0}
          >
            {brandKits.length === 0 ? (
              <option value="">No brand kits in this project</option>
            ) : null}
            {brandKits.map((k) => (
              <option key={k.id} value={k.id}>
                {k.name}
              </option>
            ))}
          </select>
        </fieldset>
      )}

      <fieldset style={fieldsetStyle}>
        <legend style={legendStyle}>Card metadata (optional)</legend>
        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <label style={fieldLabelStyle}>
            Preset label
            <input
              type="text"
              value={preset}
              onChange={(e) => setPreset(e.target.value)}
              placeholder='e.g. "PNG @1x"'
              style={inputStyle}
              maxLength={64}
            />
          </label>
          {format !== "brandKit" ? (
            <label style={fieldLabelStyle}>
              Artboard name
              <input
                type="text"
                value={artboardName}
                onChange={(e) => setArtboardName(e.target.value)}
                placeholder="(leave blank for whole document)"
                style={inputStyle}
                maxLength={120}
              />
            </label>
          ) : null}
        </div>
      </fieldset>

      <button
        type="button"
        onClick={() => {
          void handlePublish();
        }}
        disabled={busy || conversationId == null}
        style={primaryButtonStyle(busy)}
      >
        {busy
          ? "Publishing…"
          : activeConversation != null
            ? `Publish to #${activeConversation.name}`
            : "Publish"}
      </button>

      {error != null ? (
        <ErrorToast message={error} onDismiss={() => setError(null)} />
      ) : null}

      <section style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        <strong style={{ fontSize: 12 }}>Recent artifacts</strong>
        {artifacts.length === 0 ? (
          <small style={{ color: colors.textMuted }}>
            {conversationId == null
              ? "Pick a conversation to see published artifacts."
              : "No artifacts published to this conversation yet."}
          </small>
        ) : (
          <ul
            style={{
              listStyle: "none",
              margin: 0,
              padding: 0,
              display: "flex",
              flexDirection: "column",
              gap: 6,
              maxHeight: 320,
              overflowY: "auto",
            }}
          >
            {artifacts.map((a) => (
              <ArtifactRow key={a.artifactId} artifact={a} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

interface FormatStates {
  png: PngState;
  svg: SvgState;
  pdf: PdfState;
  webp: WebpState;
  jpeg: JpegState;
}

function buildArtifactRequest(
  format: Exclude<FormatChoice, "brandKit">,
  s: FormatStates,
): KChatArtifactPublishRequest["kind"] {
  switch (format) {
    case "png": {
      const opts: PngExportOptions = {
        width: s.png.width,
        height: s.png.height,
        scale: s.png.scale,
        background: null,
      };
      return { format: "png", ...opts };
    }
    case "svg": {
      const opts: SvgExportOptions = {
        width: s.svg.width,
        height: s.svg.height,
        includeMetadata: true,
        optimize: s.svg.optimize,
      };
      return { format: "svg", options: opts };
    }
    case "pdf": {
      const opts: PdfExportOptions = {
        widthMm: s.pdf.widthMm,
        heightMm: s.pdf.heightMm,
        title: s.pdf.title,
      };
      return { format: "pdf", ...opts };
    }
    case "webp": {
      const opts: WebpExportOptions = {
        width: s.webp.width,
        height: s.webp.height,
        scale: s.webp.scale,
        quality: s.webp.quality,
        lossless: s.webp.lossless,
        background: null,
      };
      return { format: "webp", ...opts };
    }
    case "jpeg": {
      const opts: JpegExportOptions = {
        width: s.jpeg.width,
        height: s.jpeg.height,
        scale: s.jpeg.scale,
        quality: s.jpeg.quality,
        background: null,
      };
      return { format: "jpeg", ...opts };
    }
    default: {
      const never: never = format;
      throw new Error(`unhandled format: ${String(never)}`);
    }
  }
}

function PngControls({
  value,
  onChange,
}: {
  value: PngState;
  onChange: (v: PngState) => void;
}): JSX.Element {
  return (
    <>
      <NumberField
        label="Width (px)"
        value={value.width}
        min={1}
        max={32768}
        onChange={(width) => onChange({ ...value, width })}
      />
      <NumberField
        label="Height (px)"
        value={value.height}
        min={1}
        max={32768}
        onChange={(height) => onChange({ ...value, height })}
      />
      <NumberField
        label="Scale"
        value={value.scale}
        min={0.1}
        max={8}
        step={0.1}
        onChange={(scale) => onChange({ ...value, scale })}
      />
    </>
  );
}

function SvgControls({
  value,
  onChange,
}: {
  value: SvgState;
  onChange: (v: SvgState) => void;
}): JSX.Element {
  return (
    <>
      <NumberField
        label="Width (px)"
        value={value.width}
        min={1}
        max={32768}
        onChange={(width) => onChange({ ...value, width })}
      />
      <NumberField
        label="Height (px)"
        value={value.height}
        min={1}
        max={32768}
        onChange={(height) => onChange({ ...value, height })}
      />
      <label style={inlineCheckboxStyle}>
        <input
          type="checkbox"
          checked={value.optimize}
          onChange={(e) => onChange({ ...value, optimize: e.target.checked })}
        />
        Optimise (svgo)
      </label>
    </>
  );
}

function PdfControls({
  value,
  onChange,
}: {
  value: PdfState;
  onChange: (v: PdfState) => void;
}): JSX.Element {
  return (
    <>
      <NumberField
        label="Width (mm)"
        value={value.widthMm}
        min={1}
        max={2000}
        onChange={(widthMm) => onChange({ ...value, widthMm })}
      />
      <NumberField
        label="Height (mm)"
        value={value.heightMm}
        min={1}
        max={2000}
        onChange={(heightMm) => onChange({ ...value, heightMm })}
      />
      <label style={fieldLabelStyle}>
        Title
        <input
          type="text"
          value={value.title}
          onChange={(e) => onChange({ ...value, title: e.target.value })}
          style={inputStyle}
          maxLength={200}
        />
      </label>
    </>
  );
}

function WebpControls({
  value,
  onChange,
}: {
  value: WebpState;
  onChange: (v: WebpState) => void;
}): JSX.Element {
  return (
    <>
      <NumberField
        label="Width (px)"
        value={value.width}
        min={1}
        max={32768}
        onChange={(width) => onChange({ ...value, width })}
      />
      <NumberField
        label="Height (px)"
        value={value.height}
        min={1}
        max={32768}
        onChange={(height) => onChange({ ...value, height })}
      />
      <NumberField
        label="Scale"
        value={value.scale}
        min={0.1}
        max={8}
        step={0.1}
        onChange={(scale) => onChange({ ...value, scale })}
      />
      <NumberField
        label="Quality"
        value={value.quality}
        min={0}
        max={100}
        onChange={(quality) => onChange({ ...value, quality })}
      />
      <label style={inlineCheckboxStyle}>
        <input
          type="checkbox"
          checked={value.lossless}
          onChange={(e) => onChange({ ...value, lossless: e.target.checked })}
        />
        Lossless (overrides quality)
      </label>
    </>
  );
}

function JpegControls({
  value,
  onChange,
}: {
  value: JpegState;
  onChange: (v: JpegState) => void;
}): JSX.Element {
  return (
    <>
      <NumberField
        label="Width (px)"
        value={value.width}
        min={1}
        max={32768}
        onChange={(width) => onChange({ ...value, width })}
      />
      <NumberField
        label="Height (px)"
        value={value.height}
        min={1}
        max={32768}
        onChange={(height) => onChange({ ...value, height })}
      />
      <NumberField
        label="Scale"
        value={value.scale}
        min={0.1}
        max={8}
        step={0.1}
        onChange={(scale) => onChange({ ...value, scale })}
      />
      <NumberField
        label="Quality"
        value={value.quality}
        min={0}
        max={100}
        onChange={(quality) => onChange({ ...value, quality })}
      />
    </>
  );
}

function NumberField({
  label,
  value,
  min,
  max,
  step,
  onChange,
}: {
  label: string;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (v: number) => void;
}): JSX.Element {
  return (
    <label style={fieldLabelStyle}>
      {label}
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        step={step ?? 1}
        onChange={(e) => {
          const parsed = Number(e.target.value);
          if (Number.isFinite(parsed)) onChange(parsed);
        }}
        style={inputStyle}
      />
    </label>
  );
}

function ArtifactRow({
  artifact,
}: {
  artifact: KChatPublishedArtifact;
}): JSX.Element {
  return (
    <li
      style={{
        display: "flex",
        gap: spacing.sm,
        padding: spacing.xs,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
        background: colors.bgSoft,
      }}
    >
      <ArtifactThumbnail artifact={artifact} />
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 2,
          flex: 1,
          minWidth: 0,
        }}
      >
        <strong
          style={{
            fontSize: 11,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            color: colors.text,
          }}
        >
          {artifact.metadata.artboardName ??
            artifact.metadata.projectName}
        </strong>
        <small style={{ color: colors.textMuted, fontSize: 10 }}>
          {artifactKindLabel(artifact.kind)} · {formatBytes(artifact.byteSize)} ·{" "}
          {formatTimestamp(artifact.publishedAt)}
        </small>
        {artifact.metadata.exportPreset != null ? (
          <small
            style={{
              fontSize: 10,
              color: colors.textMuted,
              fontFamily: "monospace",
            }}
          >
            {artifact.metadata.exportPreset}
          </small>
        ) : null}
        <div style={{ display: "flex", gap: 4 }}>
          <a
            href={artifact.previewUrl}
            target="_blank"
            rel="noreferrer"
            style={linkStyle}
          >
            Preview
          </a>
          <a
            href={artifact.thumbnailUrl}
            target="_blank"
            rel="noreferrer"
            style={linkStyle}
          >
            Thumbnail
          </a>
        </div>
      </div>
    </li>
  );
}

function ArtifactThumbnail({
  artifact,
}: {
  artifact: KChatPublishedArtifact;
}): JSX.Element {
  const [errored, setErrored] = useState(false);
  if (errored || artifact.thumbnailUrl.length === 0) {
    return (
      <div
        style={{
          width: 48,
          height: 48,
          borderRadius: radius.sm,
          background: colors.bg,
          border: `1px solid ${colors.border}`,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 9,
          color: colors.textMuted,
        }}
        aria-label="No preview"
      >
        {artifactKindLabel(artifact.kind)}
      </div>
    );
  }
  return (
    <img
      src={artifact.thumbnailUrl}
      alt={`${artifact.kind} thumbnail`}
      onError={() => setErrored(true)}
      style={{
        width: 48,
        height: 48,
        objectFit: "cover",
        borderRadius: radius.sm,
        border: `1px solid ${colors.border}`,
      }}
    />
  );
}

function ErrorToast({
  message,
  onDismiss,
}: {
  message: string;
  onDismiss: () => void;
}): JSX.Element {
  return (
    <div
      style={{
        background: colors.dangerBgSoft,
        border: `1px solid ${colors.dangerBorder}`,
        color: colors.danger,
        padding: spacing.xs,
        borderRadius: radius.sm,
        display: "flex",
        gap: 4,
        justifyContent: "space-between",
        alignItems: "center",
        fontSize: 11,
      }}
    >
      <span>{message}</span>
      <button
        type="button"
        onClick={onDismiss}
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
  );
}

function artifactKindLabel(kind: KChatArtifactKind): string {
  if (kind === "brandKit") return "Brand kit";
  return kind.toUpperCase();
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatTimestamp(iso: string): string {
  try {
    const dt = new Date(iso);
    if (Number.isNaN(dt.getTime())) return iso;
    return dt.toLocaleString();
  } catch {
    return iso;
  }
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

const inputStyle: React.CSSProperties = {
  width: "100%",
  padding: 6,
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
  boxSizing: "border-box",
};

const selectStyle: React.CSSProperties = {
  ...inputStyle,
  padding: 4,
};

const fieldLabelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  fontSize: 11,
  fontWeight: 600,
  color: colors.textMuted,
};

const fieldsetStyle: React.CSSProperties = {
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
  padding: spacing.sm,
  margin: 0,
};

const legendStyle: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 600,
  color: colors.textMuted,
  padding: "0 4px",
};

const inlineCheckboxStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  fontSize: 11,
  color: colors.text,
};

const linkStyle: React.CSSProperties = {
  color: colors.accent,
  textDecoration: "none",
  fontSize: 10,
  fontWeight: 600,
};

function primaryButtonStyle(busy: boolean): React.CSSProperties {
  return {
    padding: "8px 12px",
    fontSize: 12,
    fontWeight: 600,
    background: busy ? colors.bgSoft : colors.accent,
    color: busy ? colors.textMuted : colors.textInverse,
    border: `1px solid ${busy ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: busy ? "wait" : "pointer",
  };
}

function chipButtonStyle(active: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    fontSize: 11,
    fontWeight: 600,
    background: active ? colors.accent : "transparent",
    color: active ? colors.textInverse : colors.text,
    border: `1px solid ${active ? colors.accent : colors.border}`,
    borderRadius: radius.pill,
    cursor: "pointer",
  };
}
