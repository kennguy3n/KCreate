// Theme / Brand Kit instant restyle panel (G4).
//
// Mirrors Gamma's "switch theme to restyle the whole deck" and Canva's
// Brand Kit (pinned palette + fonts). The panel lets the user:
//
//   * pick one of the built-in themes (loaded from
//     `window.kcreate.theme.listBuiltins()`), previewing its palette
//     swatches + type scale;
//   * derive a brand-new theme from the colors already in the open
//     document (`theme.deriveFromDocument`);
//   * author a custom Brand Kit (palette + fonts) that persists with
//     the project through the canonical `brandKit.*` CRUD surface;
//   * hit **Apply** to restyle the whole document in a single undoable
//     operation (`theme.apply`) — one Ctrl+Z reverts the entire
//     restyle.
//
// The panel never touches the scene graph directly: applying a theme
// runs entirely in the Rust bridge (role-aware recolor + type-scale +
// radii), pushes a fresh frame to the canvas via scene-sync, and the
// host re-fetches the document tree through `onApplied`.
//
// Conventions (colors / radius / spacing tokens, `onStatus` bubbling,
// `errMsg`, useCallback/useEffect load+commit, small field
// sub-components) mirror `ColorSettingsPanel.tsx`.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  ApplyThemeReport,
  BrandKit,
  FontRef,
  NamedColor,
  RgbaColor,
  Theme,
  ThemePalette,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ThemePanelProps {
  /** Bubbles a transient status line to the editor footer. */
  onStatus?: (msg: string | null) => void;
  /**
   * Fired after a successful `apply` so the host can re-fetch the
   * document tree / selection / status. The canvas itself updates
   * independently via the bridge's scene-sync push, so this is only
   * about keeping React state (layer tree, properties) in sync.
   */
  onApplied?: () => void;
  /**
   * The host's current multi-selection (node ids). Used by the
   * apply-scope toggle: when scope is "selection" these roots (plus
   * their descendants) are the only nodes restyled. Empty disables the
   * selection scope.
   */
  selectedIds?: string[];
}

/** Palette roles in a stable, human-meaningful order for the swatch row. */
const PALETTE_ROLES: ReadonlyArray<{ key: keyof ThemePalette; label: string }> =
  [
    { key: "background", label: "Background" },
    { key: "surface", label: "Surface" },
    { key: "primary", label: "Primary" },
    { key: "secondary", label: "Secondary" },
    { key: "accent", label: "Accent" },
    { key: "text", label: "Text" },
    { key: "muted", label: "Muted" },
  ];

const clamp01 = (v: number): number => (v < 0 ? 0 : v > 1 ? 1 : v);
const to255 = (v: number): number => Math.round(clamp01(v) * 255);

/** `RgbaColor` (0..1 floats) → CSS `rgba(...)` for previews. */
function rgbaToCss(c: RgbaColor): string {
  return `rgba(${to255(c.r)}, ${to255(c.g)}, ${to255(c.b)}, ${clamp01(c.a)})`;
}

/** `RgbaColor` → `#rrggbb` for `<input type="color">` value binding. */
function rgbaToHex(c: RgbaColor): string {
  const h = (v: number): string => to255(v).toString(16).padStart(2, "0");
  return `#${h(c.r)}${h(c.g)}${h(c.b)}`;
}

/** `#rrggbb` → `RgbaColor`, preserving the supplied alpha. */
function hexToRgba(hex: string, alpha: number): RgbaColor {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  const digits = m?.[1];
  if (digits === undefined) return { r: 0, g: 0, b: 0, a: clamp01(alpha) };
  const int = Number.parseInt(digits, 16);
  return {
    r: ((int >> 16) & 0xff) / 255,
    g: ((int >> 8) & 0xff) / 255,
    b: (int & 0xff) / 255,
    a: clamp01(alpha),
  };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}

/**
 * Open the OS file picker for a single file and resolve its bytes (or
 * `null` if the user cancels). The renderer is sandboxed, so a transient
 * `<input type="file">` is the canonical way to read local bytes without
 * a main-process dialog round-trip (matches `ScreenshotToLayout`'s
 * `file.arrayBuffer()` flow). The `focus` fallback resolves `null` on
 * cancel — the dialog closing refocuses the window without firing
 * `change` — and the deferral lets a real selection win the race.
 */
function pickFileBytes(
  accept: string,
): Promise<{ name: string; bytes: Uint8Array } | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    let settled = false;
    // Flipped synchronously the instant a file is chosen — i.e. before
    // the async `arrayBuffer()` read settles. The cancel fallback below
    // consults this so a real selection whose read outlives the 400ms
    // timeout is never clobbered with `null` (the `focus` event can
    // fire before `change`, and a large file's read can take longer
    // than the timeout).
    let selectionStarted = false;
    const finish = (
      value: { name: string; bytes: Uint8Array } | null,
    ): void => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    input.onchange = (): void => {
      selectionStarted = true;
      const file = input.files?.[0];
      if (!file) {
        finish(null);
        return;
      }
      void file
        .arrayBuffer()
        .then((buf) => {
          finish({ name: file.name, bytes: new Uint8Array(buf) });
        })
        .catch(() => finish(null));
    };
    window.addEventListener(
      "focus",
      () => {
        window.setTimeout(() => {
          // Treat the refocus as a cancel ONLY when no selection has
          // begun; once `change` has fired, the read settles `finish`
          // on its own (success or `.catch`), so bailing here would
          // race a slow read to a spurious `null`.
          if (!selectionStarted) finish(null);
        }, 400);
      },
      { once: true },
    );
    input.click();
  });
}

export function ThemePanel({
  onStatus,
  onApplied,
  selectedIds = [],
}: ThemePanelProps): JSX.Element {
  const [builtins, setBuiltins] = useState<Theme[]>([]);
  // Themes derived from the open document this session (shown first).
  const [derived, setDerived] = useState<Theme[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [deriveName, setDeriveName] = useState("Derived theme");
  const [imageThemeName, setImageThemeName] = useState("Image theme");

  const [kits, setKits] = useState<BrandKit[]>([]);
  // The brand kit currently open in the editor (a working draft copy).
  const [draftKit, setDraftKit] = useState<BrandKit | null>(null);
  // Brand kits saved to the cross-project on-disk registry.
  const [registryKits, setRegistryKits] = useState<BrandKit[]>([]);
  // fontdb-discovered system fonts for the heading/body role pickers.
  const [systemFonts, setSystemFonts] = useState<string[]>([]);
  // Embed the chosen face into the project so exports carry it offline.
  const [embedFonts, setEmbedFonts] = useState(true);

  // Apply scope: whole document vs. the live canvas selection subtree.
  const [scope, setScope] = useState<"document" | "selection">("document");

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ApplyThemeReport | null>(null);

  // Selection scope needs at least one selected node to mean anything.
  const selectionScopeBlocked = scope === "selection" && selectedIds.length === 0;

  const allThemes = useMemo<Theme[]>(
    () => [...derived, ...builtins],
    [derived, builtins],
  );
  const selected = useMemo<Theme | null>(
    () => allThemes.find((t) => t.id === selectedId) ?? null,
    [allThemes, selectedId],
  );

  // Latest `onStatus` reachable from stable callbacks without listing it
  // as a dependency. The host re-creates `onStatus` on most renders, so
  // capturing it by ref keeps loaders (which run in the mount effect)
  // from re-firing on every parent render.
  const onStatusRef = useRef(onStatus);
  useEffect(() => {
    onStatusRef.current = onStatus;
  }, [onStatus]);

  const loadThemes = useCallback(async () => {
    const list = (await window.kcreate.theme.listBuiltins()) ?? [];
    setBuiltins(list);
    setSelectedId((prev) =>
      prev !== null ? prev : (list[0]?.id ?? null),
    );
  }, []);

  const loadKits = useCallback(async () => {
    const list = (await window.kcreate.brandKit.list()) ?? [];
    setKits(list);
  }, []);

  const loadRegistry = useCallback(async () => {
    const list = (await window.kcreate.brandKit.registryList()) ?? [];
    setRegistryKits(list);
  }, []);

  const loadFonts = useCallback(async () => {
    // Font enumeration failing is non-fatal: the role pickers still let
    // the user keep the kit's current family, and the shaper falls back
    // to its registered default when a family can't be resolved.
    try {
      const list = await window.kcreate.text.listFonts();
      setSystemFonts(list);
    } catch (e) {
      // Read `onStatus` through the ref so this loader keeps a STABLE
      // identity ([] deps). The parent may hand us a fresh `onStatus`
      // every render; listing it as a dep would re-create `loadFonts`,
      // re-fire the mount effect, and re-run every loader (incl. a fresh
      // font enumeration) on each render.
      onStatusRef.current?.(`Theme: font enumeration failed — ${errMsg(e)}`);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        await Promise.all([loadThemes(), loadKits(), loadRegistry(), loadFonts()]);
      } catch (e) {
        setError(errMsg(e));
      }
    })();
  }, [loadThemes, loadKits, loadRegistry, loadFonts]);

  // Core restyle step: announce the "applying…" status, invoke the
  // bridge, surface the success report, and notify the host. It does
  // NOT manage the busy/error lifecycle and lets failures propagate,
  // so it composes cleanly inside both the standalone `applyTheme`
  // path and the two-step `handleApplyKit` path (derive → apply)
  // without double-managing `busy` or swallowing the error — each
  // caller owns its own try/catch/finally and surfaces a
  // context-specific failure label.
  const performApply = useCallback(
    async (theme: Theme, statusLabel?: string): Promise<void> => {
      const toSelection = scope === "selection";
      onStatus?.(
        statusLabel ??
          `Theme: applying “${theme.name}”${toSelection ? " to selection" : ""}…`,
      );
      // Scope routes to the dedicated bridge entry point. Both are a
      // single undoable Operation; the selection path restyles only the
      // chosen roots + their descendants and a single Ctrl+Z reverts it.
      const r = toSelection
        ? await window.kcreate.theme.applyToSelection(theme, selectedIds)
        : await window.kcreate.theme.apply(theme);
      setReport(r);
      onStatus?.(
        `Applied “${r.themeName}”: ${r.affectedNodes} nodes — ` +
          `${r.recoloredFills} fills, ${r.recoloredStrokes} strokes, ` +
          `${r.restyledText} text.`,
      );
      onApplied?.();
    },
    [onStatus, onApplied, scope, selectedIds],
  );

  const applyTheme = useCallback(
    async (theme: Theme, statusLabel?: string): Promise<void> => {
      setBusy(true);
      setError(null);
      try {
        await performApply(theme, statusLabel);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Theme apply failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [performApply, onStatus],
  );

  const handleApplySelected = useCallback(() => {
    if (selected === null) return;
    if (selectionScopeBlocked) {
      setError(
        "Select one or more nodes on the canvas to restyle a selection, " +
          "or switch the scope to “Whole document”.",
      );
      return;
    }
    void applyTheme(selected);
  }, [selected, applyTheme, selectionScopeBlocked]);

  const handleDerive = useCallback(() => {
    void (async () => {
      setBusy(true);
      setError(null);
      const name = deriveName.trim() === "" ? "Derived theme" : deriveName.trim();
      onStatus?.(`Theme: deriving “${name}” from this design…`);
      try {
        const t = await window.kcreate.theme.deriveFromDocument(name);
        setDerived((prev) => [t, ...prev.filter((p) => p.id !== t.id)]);
        setSelectedId(t.id);
        onStatus?.(`Derived theme “${t.name}” — review and Apply.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Derive failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [deriveName, onStatus]);

  // --- Brand-kit authoring -------------------------------------------------

  const openKit = useCallback((kit: BrandKit) => {
    // Deep-copy so edits stay local until Save.
    setDraftKit({
      ...kit,
      colors: kit.colors.map((c) => ({ ...c, color: { ...c.color } })),
      fonts: kit.fonts.map((f) => ({ ...f })),
      spacing_scale: [...kit.spacing_scale],
      export_rules: kit.export_rules.map((r) => ({ ...r })),
    });
  }, []);

  const handleNewKit = useCallback(() => {
    void (async () => {
      setBusy(true);
      setError(null);
      const name = `Brand kit ${kits.length + 1}`;
      try {
        const id = await window.kcreate.brandKit.create(name);
        await window.kcreate.document.saveProject();
        const list = (await window.kcreate.brandKit.list()) ?? [];
        setKits(list);
        const created = list.find((k) => k.id === id) ?? null;
        if (created !== null) openKit(created);
        onStatus?.(`Created brand kit “${name}”.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Create kit failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [kits.length, openKit, onStatus]);

  const handleSaveKit = useCallback(() => {
    if (draftKit === null) return;
    void (async () => {
      setBusy(true);
      setError(null);
      try {
        await window.kcreate.brandKit.update(draftKit);
        await window.kcreate.document.saveProject();
        await loadKits();
        onStatus?.(`Saved brand kit “${draftKit.name}”.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Save kit failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [draftKit, loadKits, onStatus]);

  const handleDeleteKit = useCallback(
    (kitId: string) => {
      void (async () => {
        setBusy(true);
        setError(null);
        try {
          await window.kcreate.brandKit.delete(kitId);
          await window.kcreate.document.saveProject();
          await loadKits();
          setDraftKit((prev) => (prev?.id === kitId ? null : prev));
          onStatus?.("Deleted brand kit.");
        } catch (e) {
          const msg = errMsg(e);
          setError(msg);
          onStatus?.(`Delete kit failed: ${msg}`);
        } finally {
          setBusy(false);
        }
      })();
    },
    [loadKits, onStatus],
  );

  const handleApplyKit = useCallback(
    (kit: BrandKit) => {
      // Brand-kit applies honour the scope toggle exactly like the main
      // "Apply theme" button, so guard the empty-selection case here too
      // rather than letting performApply route to a zero-node no-op.
      if (selectionScopeBlocked) {
        setError(
          "Select one or more nodes on the canvas to restyle a selection, " +
            "or switch the scope to “Whole document”.",
        );
        return;
      }
      void (async () => {
        const label = `Theme: applying brand kit “${kit.name}”…`;
        setBusy(true);
        setError(null);
        onStatus?.(label);
        try {
          const theme = await window.kcreate.theme.fromBrandKit(kit);
          // Compose the non-lifecycle-managing core directly (rather
          // than `applyTheme`) so this two-step op keeps a single
          // busy/error scope and ANY failure — the derive OR the
          // apply — surfaces the kit-specific label below instead of
          // applyTheme's generic "Theme apply failed". The `label`
          // pass-through also keeps the kit-specific "applying…"
          // status from being clobbered by the per-theme default.
          await performApply(theme, label);
        } catch (e) {
          const msg = errMsg(e);
          setError(msg);
          onStatus?.(`Apply kit failed: ${msg}`);
        } finally {
          setBusy(false);
        }
      })();
    },
    [performApply, onStatus, selectionScopeBlocked],
  );

  // Draft mutators (operate on the working copy; persisted on Save).
  const patchDraft = useCallback((next: Partial<BrandKit>) => {
    setDraftKit((prev) => (prev === null ? prev : { ...prev, ...next }));
  }, []);

  const addColor = useCallback(() => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            colors: [
              ...prev.colors,
              {
                name: `Color ${prev.colors.length + 1}`,
                color: { r: 0.15, g: 0.39, b: 0.92, a: 1 },
              },
            ],
          },
    );
  }, []);

  const updateColor = useCallback((index: number, next: NamedColor) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            colors: prev.colors.map((c, i) => (i === index ? next : c)),
          },
    );
  }, []);

  const removeColor = useCallback((index: number) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : { ...prev, colors: prev.colors.filter((_, i) => i !== index) },
    );
  }, []);

  const addFont = useCallback(() => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            fonts: [
              ...prev.fonts,
              { family: "Inter", weight: 400, italic: false, embedded_asset_id: null },
            ],
          },
    );
  }, []);

  const updateFont = useCallback((index: number, next: FontRef) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : { ...prev, fonts: prev.fonts.map((f, i) => (i === index ? next : f)) },
    );
  }, []);

  const removeFont = useCallback((index: number) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : { ...prev, fonts: prev.fonts.filter((_, i) => i !== index) },
    );
  }, []);

  // --- Brand-kit asset mutations (logo / palette / fonts) ------------------
  //
  // These bridge calls mutate the PERSISTED project kit (they read it from
  // the workspace by id, not from our local draft), so the draft must be
  // flushed first or unsaved edits would be lost when we reload. The shared
  // runner: flush draft → run the mutation → saveProject → reload + reopen
  // by id to pick up the new asset refs / replaced palette the bridge wrote.
  const runKitMutation = useCallback(
    async (label: string, mutate: (kitId: string) => Promise<void>): Promise<void> => {
      if (draftKit === null) return;
      const kitId = draftKit.id;
      setBusy(true);
      setError(null);
      onStatus?.(`${label}…`);
      try {
        await window.kcreate.brandKit.update(draftKit);
        await mutate(kitId);
        await window.kcreate.document.saveProject();
        const list = (await window.kcreate.brandKit.list()) ?? [];
        setKits(list);
        const refreshed = list.find((k) => k.id === kitId) ?? null;
        if (refreshed !== null) openKit(refreshed);
        onStatus?.(`${label} — done.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`${label} failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [draftKit, openKit, onStatus],
  );

  const handleImportLogo = useCallback(() => {
    if (draftKit === null) return;
    void (async () => {
      const picked = await pickFileBytes(
        "image/svg+xml,image/png,image/jpeg,image/webp,.svg",
      );
      if (picked === null) return;
      await runKitMutation(`Setting logo from “${picked.name}”`, (id) =>
        window.kcreate.brandKit.setLogoBytes(id, picked.bytes),
      );
    })();
  }, [draftKit, runKitMutation]);

  const handlePaletteFromImage = useCallback(() => {
    if (draftKit === null) return;
    void (async () => {
      const picked = await pickFileBytes("image/png,image/jpeg,image/webp");
      if (picked === null) return;
      await runKitMutation(
        `Extracting palette from “${picked.name}”`,
        async (id) => {
          await window.kcreate.brandKit.extractPaletteFromImage(
            id,
            picked.bytes,
            6,
          );
        },
      );
    })();
  }, [draftKit, runKitMutation]);

  const handleSetRoleFont = useCallback(
    (role: "heading" | "body", family: string) => {
      if (draftKit === null || family === "") return;
      void runKitMutation(`Setting ${role} font to ${family}`, (id) =>
        window.kcreate.brandKit.setFontRole(id, role, family, embedFonts),
      );
    },
    [draftKit, embedFonts, runKitMutation],
  );

  const handleInsertLogo = useCallback(() => {
    if (draftKit === null) return;
    const kitId = draftKit.id;
    if (draftKit.logo_asset_id === null) {
      setError(
        "This brand kit has no logo yet — import an SVG or raster logo first.",
      );
      return;
    }
    void (async () => {
      setBusy(true);
      setError(null);
      onStatus?.("Inserting brand logo…");
      try {
        const placed = await window.kcreate.brandKit.insertLogo(
          kitId,
          null,
          48,
          48,
          160,
        );
        onApplied?.();
        onStatus?.(
          `Inserted brand logo (${Math.round(placed.width)}×${Math.round(
            placed.height,
          )}).`,
        );
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Insert logo failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [draftKit, onApplied, onStatus]);

  const handleDeriveFromImage = useCallback(() => {
    void (async () => {
      const picked = await pickFileBytes("image/png,image/jpeg,image/webp");
      if (picked === null) return;
      setBusy(true);
      setError(null);
      const name =
        imageThemeName.trim() === "" ? "Image theme" : imageThemeName.trim();
      onStatus?.(`Theme: deriving “${name}” from ${picked.name}…`);
      try {
        const t = await window.kcreate.theme.deriveFromImage(name, picked.bytes);
        setDerived((prev) => [t, ...prev.filter((p) => p.id !== t.id)]);
        setSelectedId(t.id);
        onStatus?.(`Derived theme “${t.name}” from image — review and Apply.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Derive from image failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [imageThemeName, onStatus]);

  // --- Cross-project brand library (on-disk registry) ----------------------

  const handleSaveToLibrary = useCallback(() => {
    if (draftKit === null) return;
    const kitId = draftKit.id;
    const draftName = draftKit.name;
    const snapshot = draftKit;
    void (async () => {
      setBusy(true);
      setError(null);
      onStatus?.(`Saving “${draftName}” to the brand library…`);
      try {
        await window.kcreate.brandKit.update(snapshot);
        await window.kcreate.document.saveProject();
        await window.kcreate.brandKit.registrySave(kitId);
        await loadRegistry();
        onStatus?.(`Saved “${draftName}” to the brand library.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Save to library failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [draftKit, loadRegistry, onStatus]);

  const handleLoadFromLibrary = useCallback(
    (kit: BrandKit) => {
      void (async () => {
        setBusy(true);
        setError(null);
        onStatus?.(`Loading “${kit.name}” into this project…`);
        try {
          const newId = await window.kcreate.brandKit.registryLoad(kit.id);
          await window.kcreate.document.saveProject();
          const list = (await window.kcreate.brandKit.list()) ?? [];
          setKits(list);
          const loaded = list.find((k) => k.id === newId) ?? null;
          if (loaded !== null) openKit(loaded);
          onStatus?.(`Loaded “${kit.name}” into this project.`);
        } catch (e) {
          const msg = errMsg(e);
          setError(msg);
          onStatus?.(`Load from library failed: ${msg}`);
        } finally {
          setBusy(false);
        }
      })();
    },
    [openKit, onStatus],
  );

  const handleDeleteFromLibrary = useCallback(
    (kitId: string) => {
      void (async () => {
        setBusy(true);
        setError(null);
        try {
          await window.kcreate.brandKit.registryDelete(kitId);
          await loadRegistry();
          onStatus?.("Removed kit from the brand library.");
        } catch (e) {
          const msg = errMsg(e);
          setError(msg);
          onStatus?.(`Remove from library failed: ${msg}`);
        } finally {
          setBusy(false);
        }
      })();
    },
    [loadRegistry, onStatus],
  );

  // Current heading/body family for the role pickers (heading = weight ≥ 600).
  const headingFont = draftKit?.fonts.find((f) => f.weight >= 600)?.family ?? "";
  const bodyFont = draftKit?.fonts.find((f) => f.weight < 600)?.family ?? "";
  const fontOptions = useMemo<string[]>(() => {
    const set = new Set<string>(systemFonts);
    if (headingFont !== "") set.add(headingFont);
    if (bodyFont !== "") set.add(bodyFont);
    return [...set].sort((a, b) => a.localeCompare(b));
  }, [systemFonts, headingFont, bodyFont]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        fontSize: 12,
        color: colors.text,
      }}
    >
      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Themes</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          {scope === "selection"
            ? "Applying restyles the selected nodes (and their children) in one undoable step."
            : "Applying a theme restyles the whole document in one undoable step."}
        </p>
        <div
          role="radiogroup"
          aria-label="Apply scope"
          style={{ display: "flex", gap: spacing.xs }}
        >
          <button
            type="button"
            role="radio"
            aria-checked={scope === "document"}
            onClick={() => setScope("document")}
            style={segmentButtonStyle(scope === "document")}
          >
            Whole document
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={scope === "selection"}
            onClick={() => setScope("selection")}
            style={segmentButtonStyle(scope === "selection")}
          >
            Selection ({selectedIds.length})
          </button>
        </div>
        {selectionScopeBlocked ? (
          <span style={{ color: colors.textMuted, fontSize: 11 }}>
            Select one or more nodes on the canvas to enable selection scope.
          </span>
        ) : null}
        <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
          {allThemes.map((theme) => (
            <ThemeCard
              key={theme.id}
              theme={theme}
              selected={theme.id === selectedId}
              onSelect={() => setSelectedId(theme.id)}
            />
          ))}
          {allThemes.length === 0 ? (
            <span style={{ color: colors.textMuted, fontSize: 11 }}>
              No themes available.
            </span>
          ) : null}
        </div>
        <button
          type="button"
          disabled={busy || selected === null || selectionScopeBlocked}
          onClick={handleApplySelected}
          aria-label="Apply theme"
          style={primaryButtonStyle(busy || selected === null || selectionScopeBlocked)}
        >
          {busy
            ? "Applying…"
            : `Apply${selected ? ` “${selected.name}”` : ""}${
                scope === "selection" ? " to selection" : ""
              }`}
        </button>
      </section>

      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Derive from this design</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Build a theme from the colors already used in the open document.
        </p>
        <div style={{ display: "flex", gap: spacing.sm }}>
          <input
            type="text"
            value={deriveName}
            onChange={(e) => setDeriveName(e.target.value)}
            aria-label="Derived theme name"
            style={inputStyle}
          />
          <button
            type="button"
            disabled={busy}
            onClick={handleDerive}
            aria-label="Derive theme from document"
            style={secondaryButtonStyle(busy)}
          >
            Derive
          </button>
        </div>
      </section>

      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Derive from an image</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Upload a photo or artwork — its dominant colors become a new theme.
        </p>
        <div style={{ display: "flex", gap: spacing.sm }}>
          <input
            type="text"
            value={imageThemeName}
            onChange={(e) => setImageThemeName(e.target.value)}
            aria-label="Image theme name"
            style={inputStyle}
          />
          <button
            type="button"
            disabled={busy}
            onClick={handleDeriveFromImage}
            aria-label="Derive theme from image"
            style={secondaryButtonStyle(busy)}
          >
            Upload image…
          </button>
        </div>
      </section>

      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Brand kits</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Pin a custom palette + fonts. Saved with the project.
        </p>
        <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
          {kits.map((kit) => (
            <div
              key={kit.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: spacing.sm,
                padding: spacing.xs,
                border: `1px solid ${colors.border}`,
                borderRadius: radius.md,
                background:
                  draftKit?.id === kit.id ? colors.accentBgSoft : colors.bg,
              }}
            >
              <SwatchRow colorsList={kit.colors.map((c) => c.color)} />
              <span style={{ flex: 1, fontSize: 11 }}>{kit.name}</span>
              <button
                type="button"
                disabled={busy}
                onClick={() => openKit(kit)}
                aria-label={`Edit ${kit.name}`}
                style={miniButtonStyle(busy)}
              >
                Edit
              </button>
              <button
                type="button"
                disabled={busy || selectionScopeBlocked}
                onClick={() => handleApplyKit(kit)}
                aria-label={`Apply ${kit.name}`}
                style={miniButtonStyle(busy || selectionScopeBlocked)}
              >
                Apply
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => handleDeleteKit(kit.id)}
                aria-label={`Delete ${kit.name}`}
                style={miniButtonStyle(busy)}
              >
                Delete
              </button>
            </div>
          ))}
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={handleNewKit}
          aria-label="New brand kit"
          style={secondaryButtonStyle(busy)}
        >
          + New brand kit
        </button>
      </section>

      {draftKit !== null ? (
        <section
          style={{
            display: "flex",
            flexDirection: "column",
            gap: spacing.sm,
            padding: spacing.sm,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.card,
            background: colors.bgSoft,
          }}
        >
          <SectionTitle>Edit “{draftKit.name}”</SectionTitle>
          <LabeledField label="Name">
            <input
              type="text"
              value={draftKit.name}
              onChange={(e) => patchDraft({ name: e.target.value })}
              aria-label="Brand kit name"
              style={inputStyle}
            />
          </LabeledField>

          <div style={{ fontSize: 11, color: colors.textMuted }}>Logo</div>
          <div style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
            <span style={{ flex: 1, fontSize: 11, color: colors.textMuted }}>
              {draftKit.logo_asset_id !== null
                ? "Logo set — insert it as an editable node."
                : "No logo yet. Import an SVG or raster image."}
            </span>
            <button
              type="button"
              disabled={busy}
              onClick={handleImportLogo}
              aria-label="Import brand logo"
              style={miniButtonStyle(busy)}
            >
              Import…
            </button>
            <button
              type="button"
              disabled={busy || draftKit.logo_asset_id === null}
              onClick={handleInsertLogo}
              aria-label="Insert brand logo"
              style={miniButtonStyle(busy || draftKit.logo_asset_id === null)}
            >
              Insert
            </button>
          </div>

          <div style={{ fontSize: 11, color: colors.textMuted }}>Colors</div>
          {draftKit.colors.map((c, i) => (
            <div
              key={i}
              style={{ display: "flex", gap: spacing.xs, alignItems: "center" }}
            >
              <input
                type="color"
                value={rgbaToHex(c.color)}
                onChange={(e) =>
                  updateColor(i, {
                    ...c,
                    color: hexToRgba(e.target.value, c.color.a),
                  })
                }
                aria-label={`${c.name} color`}
                style={{
                  width: 28,
                  height: 24,
                  padding: 0,
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.sm,
                  background: "transparent",
                }}
              />
              <input
                type="text"
                value={c.name}
                onChange={(e) => updateColor(i, { ...c, name: e.target.value })}
                aria-label={`Color ${i + 1} name`}
                style={{ ...inputStyle, flex: 1 }}
              />
              <button
                type="button"
                onClick={() => removeColor(i)}
                aria-label={`Remove color ${i + 1}`}
                style={miniButtonStyle(false)}
              >
                ✕
              </button>
            </div>
          ))}
          <div style={{ display: "flex", gap: spacing.sm }}>
            <button
              type="button"
              onClick={addColor}
              aria-label="Add color"
              style={miniButtonStyle(false)}
            >
              + Color
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={handlePaletteFromImage}
              aria-label="Extract palette from image"
              style={miniButtonStyle(busy)}
            >
              Palette from image…
            </button>
          </div>

          <div style={{ fontSize: 11, color: colors.textMuted }}>
            Heading &amp; body fonts
          </div>
          <LabeledField label="Heading">
            <select
              value={headingFont}
              disabled={busy}
              onChange={(e) => handleSetRoleFont("heading", e.target.value)}
              aria-label="Heading font"
              style={inputStyle}
            >
              <option value="">Choose a font…</option>
              {fontOptions.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </select>
          </LabeledField>
          <LabeledField label="Body">
            <select
              value={bodyFont}
              disabled={busy}
              onChange={(e) => handleSetRoleFont("body", e.target.value)}
              aria-label="Body font"
              style={inputStyle}
            >
              <option value="">Choose a font…</option>
              {fontOptions.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </select>
          </LabeledField>
          <label
            style={{
              display: "flex",
              alignItems: "center",
              gap: spacing.xs,
              fontSize: 11,
              color: colors.textMuted,
            }}
          >
            <input
              type="checkbox"
              checked={embedFonts}
              onChange={(e) => setEmbedFonts(e.target.checked)}
              aria-label="Embed chosen fonts in the project"
            />
            Embed chosen fonts (carried into PDF/SVG exports, offline)
          </label>

          <div style={{ fontSize: 11, color: colors.textMuted }}>
            Fonts (advanced)
          </div>
          {draftKit.fonts.map((f, i) => (
            <div
              key={i}
              style={{ display: "flex", gap: spacing.xs, alignItems: "center" }}
            >
              <input
                type="text"
                value={f.family}
                onChange={(e) => updateFont(i, { ...f, family: e.target.value })}
                aria-label={`Font ${i + 1} family`}
                style={{ ...inputStyle, flex: 1 }}
              />
              <input
                type="number"
                value={f.weight}
                min={100}
                max={900}
                step={100}
                onChange={(e) =>
                  updateFont(i, {
                    ...f,
                    weight: Number.parseInt(e.target.value, 10) || 400,
                  })
                }
                aria-label={`Font ${i + 1} weight`}
                style={{ ...inputStyle, width: 64 }}
              />
              <button
                type="button"
                onClick={() => removeFont(i)}
                aria-label={`Remove font ${i + 1}`}
                style={miniButtonStyle(false)}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            type="button"
            onClick={addFont}
            aria-label="Add font"
            style={miniButtonStyle(false)}
          >
            + Font
          </button>

          <div style={{ display: "flex", flexWrap: "wrap", gap: spacing.sm }}>
            <button
              type="button"
              disabled={busy}
              onClick={handleSaveKit}
              aria-label="Save brand kit"
              style={primaryButtonStyle(busy)}
            >
              Save
            </button>
            <button
              type="button"
              disabled={busy || selectionScopeBlocked}
              onClick={() => handleApplyKit(draftKit)}
              aria-label="Apply brand kit as theme"
              style={secondaryButtonStyle(busy || selectionScopeBlocked)}
            >
              Apply as theme
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={handleSaveToLibrary}
              aria-label="Save brand kit to library"
              style={secondaryButtonStyle(busy)}
            >
              Save to library
            </button>
            <button
              type="button"
              onClick={() => setDraftKit(null)}
              aria-label="Close brand kit editor"
              style={secondaryButtonStyle(false)}
            >
              Close
            </button>
          </div>
        </section>
      ) : null}

      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Brand library</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Brand kits saved to disk and reusable across every project.
        </p>
        <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
          {registryKits.map((kit) => (
            <div
              key={kit.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: spacing.sm,
                padding: spacing.xs,
                border: `1px solid ${colors.border}`,
                borderRadius: radius.md,
                background: colors.bg,
              }}
            >
              <SwatchRow colorsList={kit.colors.map((c) => c.color)} />
              <span style={{ flex: 1, fontSize: 11 }}>{kit.name}</span>
              <button
                type="button"
                disabled={busy}
                onClick={() => handleLoadFromLibrary(kit)}
                aria-label={`Load ${kit.name} into this project`}
                style={miniButtonStyle(busy)}
              >
                Load
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => handleDeleteFromLibrary(kit.id)}
                aria-label={`Delete ${kit.name} from library`}
                style={miniButtonStyle(busy)}
              >
                Delete
              </button>
            </div>
          ))}
          {registryKits.length === 0 ? (
            <span style={{ color: colors.textMuted, fontSize: 11 }}>
              No saved kits yet. Open a kit and choose “Save to library”.
            </span>
          ) : null}
        </div>
      </section>

      {report !== null ? (
        <div style={{ fontSize: 11, color: colors.textMuted }}>
          Last apply: {report.themeName} — {report.affectedNodes} nodes,{" "}
          {report.recoloredFills} fills, {report.recoloredStrokes} strokes,{" "}
          {report.restyledText} text.
        </div>
      ) : null}

      {error !== null ? (
        <div style={{ fontSize: 11, color: colors.danger }}>{error}</div>
      ) : null}
    </div>
  );
}

// --- presentational sub-components -----------------------------------------

function SectionTitle({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div style={{ fontSize: 12, fontWeight: 600, color: colors.text }}>
      {children}
    </div>
  );
}

function LabeledField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      {children}
    </label>
  );
}

function SwatchRow({ colorsList }: { colorsList: RgbaColor[] }): JSX.Element {
  return (
    <div style={{ display: "flex", gap: 2 }}>
      {colorsList.slice(0, 7).map((c, i) => (
        <span
          key={i}
          style={{
            width: 14,
            height: 14,
            borderRadius: radius.sm,
            background: rgbaToCss(c),
            border: `1px solid ${colors.border}`,
          }}
        />
      ))}
    </div>
  );
}

function ThemeCard({
  theme,
  selected,
  onSelect,
}: {
  theme: Theme;
  selected: boolean;
  onSelect: () => void;
}): JSX.Element {
  const p = theme.palette;
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-label={`Select theme ${theme.name}`}
      aria-pressed={selected}
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
        padding: spacing.sm,
        textAlign: "left",
        cursor: "pointer",
        border: `1px solid ${selected ? colors.accent : colors.border}`,
        borderRadius: radius.card,
        background: rgbaToCss(p.background),
        outline: selected ? `2px solid ${colors.accentRing}` : "none",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
        }}
      >
        <span
          style={{
            fontFamily: theme.type_scale.heading_font,
            fontSize: 15,
            fontWeight: 700,
            color: rgbaToCss(p.text),
          }}
        >
          {theme.name}
        </span>
        <span
          style={{
            fontFamily: theme.type_scale.body_font,
            fontSize: 11,
            color: rgbaToCss(p.muted),
          }}
        >
          Aa
        </span>
      </div>
      <SwatchRow
        colorsList={PALETTE_ROLES.map(({ key }) => p[key])}
      />
    </button>
  );
}

// --- shared inline styles ---------------------------------------------------

const inputStyle: React.CSSProperties = {
  fontSize: 12,
  padding: `${spacing.xs}px ${spacing.sm}px`,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
  background: colors.bg,
  color: colors.text,
};

function primaryButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 12,
    padding: `${spacing.xs}px ${spacing.md}px`,
    borderRadius: radius.md,
    border: `1px solid ${colors.accent}`,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    cursor: disabled ? "default" : "pointer",
  };
}

function secondaryButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 12,
    padding: `${spacing.xs}px ${spacing.md}px`,
    borderRadius: radius.md,
    border: `1px solid ${colors.border}`,
    background: colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    cursor: disabled ? "default" : "pointer",
  };
}

/** A segmented-control button: highlighted when it is the active choice. */
function segmentButtonStyle(active: boolean): React.CSSProperties {
  return {
    flex: 1,
    fontSize: 11,
    padding: `${spacing.xs}px ${spacing.sm}px`,
    borderRadius: radius.md,
    border: `1px solid ${active ? colors.accent : colors.border}`,
    background: active ? colors.accentBgSoft : colors.bg,
    color: active ? colors.text : colors.textMuted,
    cursor: "pointer",
  };
}

function miniButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 11,
    padding: `2px ${spacing.sm}px`,
    borderRadius: radius.sm,
    border: `1px solid ${colors.border}`,
    background: colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    cursor: disabled ? "default" : "pointer",
  };
}
