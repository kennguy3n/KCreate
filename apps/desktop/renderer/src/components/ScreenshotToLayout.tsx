// ScreenshotToLayout — drop a screenshot, run edge-detection +
// connected-component analysis on it (`kcreate_ai::analyze_screenshot_for_layout`),
// preview detected regions, and (optionally) generate a layout
// scaffold from the chosen subset.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  ScreenshotElement,
  ScreenshotElementType,
  ScreenshotRequest,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ScreenshotToLayoutProps {
  onStatus?: (msg: string | null) => void;
}

interface LoadedImage {
  pixels: Uint8ClampedArray;
  width: number;
  height: number;
  dataUrl: string;
}

export function ScreenshotToLayout({
  onStatus,
}: ScreenshotToLayoutProps): JSX.Element {
  const [image, setImage] = useState<LoadedImage | null>(null);
  const [elements, setElements] = useState<ScreenshotElement[]>([]);
  const [included, setIncluded] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  // The object-URL the browser is currently holding a Blob reference
  // through. We track it in a ref so onFile can revoke the previous one
  // without closing over the live `image` state, and the unmount effect
  // can revoke whatever's outstanding when the component goes away.
  const objectUrlRef = useRef<string | null>(null);

  useEffect(
    () => () => {
      if (objectUrlRef.current) {
        URL.revokeObjectURL(objectUrlRef.current);
        objectUrlRef.current = null;
      }
    },
    [],
  );

  const onFile = useCallback(
    async (file: File) => {
      const arr = await file.arrayBuffer();
      const blob = new Blob([arr], { type: file.type });
      const url = URL.createObjectURL(blob);
      const img = new Image();
      img.src = url;
      await img.decode();
      const canvas = document.createElement("canvas");
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      const ctx = canvas.getContext("2d");
      if (!ctx) {
        URL.revokeObjectURL(url);
        onStatus?.("screenshot: no 2D context available.");
        return;
      }
      ctx.drawImage(img, 0, 0);
      const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);
      // Replace the outstanding object URL before adopting the new one so
      // the browser can release the previous Blob.
      if (objectUrlRef.current) {
        URL.revokeObjectURL(objectUrlRef.current);
      }
      objectUrlRef.current = url;
      setImage({
        pixels: imageData.data,
        width: canvas.width,
        height: canvas.height,
        dataUrl: url,
      });
      setElements([]);
      setIncluded(new Set());
    },
    [onStatus],
  );

  const analyze = useCallback(async () => {
    if (!image) return;
    setBusy(true);
    onStatus?.("screenshot: analyzing…");
    try {
      const b64 = await bytesToBase64(image.pixels);
      const req: ScreenshotRequest = {
        imageBase64: b64,
        width: image.width,
        height: image.height,
      };
      const detected = await window.kcreate.aiModel.screenshotToLayout(req);
      setElements(detected);
      setIncluded(new Set(detected.map((_, idx) => idx)));
      onStatus?.(`screenshot: ${detected.length} regions detected.`);
    } catch (e) {
      onStatus?.(`screenshot failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [image, onStatus]);

  const toggle = useCallback((idx: number) => {
    setIncluded((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }, []);

  const overlay = useMemo(() => {
    if (!image) return null;
    return (
      <svg
        viewBox={`0 0 ${image.width} ${image.height}`}
        style={{
          position: "absolute",
          inset: 0,
          width: "100%",
          height: "100%",
          pointerEvents: "none",
        }}
      >
        {elements.map((el, idx) => (
          <rect
            key={idx}
            x={el.bounds.x}
            y={el.bounds.y}
            width={el.bounds.width}
            height={el.bounds.height}
            fill="none"
            stroke={included.has(idx) ? colors.accent : "#999"}
            strokeWidth={Math.max(2, image.width / 400)}
            opacity={included.has(idx) ? 1 : 0.4}
          />
        ))}
      </svg>
    );
  }, [elements, image, included]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
      }}
    >
      <header>
        <h2 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>
          Screenshot → Layout
        </h2>
        <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
          Detect header / nav / hero / button / card regions from any UI
          screenshot. Runs entirely on the local CV pipeline — no network.
        </p>
      </header>
      <div
        onDragOver={(e) => {
          e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          const f = e.dataTransfer.files[0];
          if (f) void onFile(f);
        }}
        style={{
          border: `2px dashed ${colors.border}`,
          borderRadius: radius.card,
          padding: spacing.lg,
          textAlign: "center",
          cursor: "pointer",
          background: colors.bgSoft,
        }}
        onClick={() => inputRef.current?.click()}
      >
        {image ? "Drop another screenshot to replace" : "Drop screenshot or click to pick"}
        <input
          ref={inputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp"
          style={{ display: "none" }}
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void onFile(f);
          }}
        />
      </div>
      {image ? (
        <div style={{ position: "relative" }}>
          <img
            src={image.dataUrl}
            alt="screenshot"
            style={{
              maxWidth: "100%",
              border: `1px solid ${colors.border}`,
              borderRadius: radius.card,
            }}
          />
          {overlay}
        </div>
      ) : null}
      <button
        type="button"
        onClick={() => {
          void analyze();
        }}
        disabled={!image || busy}
        style={{
          padding: `${spacing.sm}px ${spacing.md}px`,
          background: !image || busy ? colors.bgSoft : colors.accent,
          color: !image || busy ? colors.textMuted : colors.textInverse,
          border: "none",
          borderRadius: radius.pill,
          fontWeight: 600,
          fontSize: 12,
          cursor: !image || busy ? "default" : "pointer",
          alignSelf: "flex-start",
        }}
      >
        {busy ? "Analyzing…" : "Detect regions"}
      </button>
      {elements.length === 0 ? null : (
        <section
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 6,
            maxHeight: 320,
            overflowY: "auto",
          }}
        >
          {elements.map((el, idx) => (
            <ElementRow
              key={idx}
              element={el}
              included={included.has(idx)}
              onToggle={() => toggle(idx)}
            />
          ))}
        </section>
      )}
    </div>
  );
}

function ElementRow({
  element,
  included,
  onToggle,
}: {
  element: ScreenshotElement;
  included: boolean;
  onToggle: () => void;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.sm,
        padding: spacing.sm,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        cursor: "pointer",
      }}
    >
      <input type="checkbox" checked={included} onChange={onToggle} />
      <TypePill type={element.element_type} />
      <span style={{ flex: 1, fontSize: 12 }}>{element.suggested_name}</span>
      <span style={{ fontSize: 10, color: colors.textMuted }}>
        {Math.round(element.bounds.width)} × {Math.round(element.bounds.height)}
      </span>
      <span style={{ fontSize: 10, color: colors.textMuted }}>
        {Math.round(element.confidence * 100)}%
      </span>
    </label>
  );
}

function TypePill({ type }: { type: ScreenshotElementType }): JSX.Element {
  return (
    <span
      style={{
        padding: "1px 6px",
        background: colors.accentBgSoft,
        color: colors.accent,
        borderRadius: radius.pill,
        fontSize: 10,
        fontWeight: 600,
        textTransform: "uppercase",
      }}
    >
      {type.replace(/_/g, " ")}
    </span>
  );
}

async function bytesToBase64(bytes: Uint8ClampedArray): Promise<string> {
  // FileReader.readAsDataURL is the lowest-overhead base64 path
  // available in the renderer. Compared to the previous chunked
  // String.fromCharCode + btoa approach it avoids the ~33 MB
  // intermediate binary string that lived alongside the raw pixel
  // buffer *and* the final base64 output — for a 4K screenshot that
  // dropped peak transient memory from ~110 MB to ~77 MB. Per Devin
  // Review ANALYSIS_pr-review-job-790e7860e5c745e0bee13295709290f4_0004.
  const blob = new Blob([bytes], { type: "application/octet-stream" });
  const dataUrl: string = await new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      resolve(reader.result as string);
    };
    reader.onerror = () => {
      reject(
        reader.error ??
          new Error("FileReader failed reading screenshot bytes to base64"),
      );
    };
    reader.readAsDataURL(blob);
  });
  // `readAsDataURL` returns e.g. `data:application/octet-stream;base64,AAAA…`.
  // Strip everything up to and including the comma.
  const comma = dataUrl.indexOf(",");
  return comma === -1 ? dataUrl : dataUrl.slice(comma + 1);
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
