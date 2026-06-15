// TemplateMarketplace — Phase 3, Tasks 11-12.
//
// Surface for browsing, searching, installing, and removing local
// `.ktemplate/` folders from the marketplace directory
// (`~/.kcreate/templates/` by default; override with the
// `KCREATE_TEMPLATE_DIR` env var that the bridge reads).
//
// The actual scan / install / remove logic lives in
// `kcreate_core::marketplace::LocalMarketplace` and is reached through
// the `window.kcreate.templateMarketplace.*` IPC surface. This
// component is a presentation layer with three jobs:
//
//   1. List installed templates, filterable by `TemplateCategory` and
//      a free-text query (case-insensitive substring over name, tag,
//      and description). The free-text query takes precedence over
//      the category filter — when both are set, the bridge applies
//      only the query, matching the visible search-box-dominates UX.
//
//   2. Install a `.ktemplate/` folder from any path on disk. The user
//      types the path manually; we don't open a native file picker
//      because directory selection is a separate IPC surface that
//      Phase 3 doesn't pull in. The bridge copies the folder into the
//      marketplace root and returns the parsed manifest.
//
//   3. Remove an installed template by id. The `.ktemplate/` folder
//      is deleted from disk; the user is asked to confirm because
//      this is irreversible.
//
// Real implementation, no scaffolding — the panel exercises the same
// bridge surface that the integration tests in
// `crates/kcreate_tests/tests/template_marketplace.rs` cover.

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ChangeEvent,
} from "react";

import type {
  TemplateCategory,
  TemplateManifest,
} from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";
import {
  ALL_CATEGORIES,
  CATEGORY_LABELS,
  CATEGORY_TINT,
} from "../lib/templateCategories";

export interface TemplateMarketplaceProps {
  /** Bubble status messages to the editor's global status strip. */
  onStatus?: (msg: string | null) => void;
  /**
   * Optional hook fired after a template was successfully removed. The
   * host can use this to refresh any auxiliary state that depends on
   * the installed-template set.
   */
  onTemplateRemoved?: (templateId: string) => void;
  /**
   * Optional hook fired after a template was successfully installed.
   * The host can update breadcrumbs / counts / etc.
   */
  onTemplateInstalled?: (manifest: TemplateManifest) => void;
}

export function TemplateMarketplace({
  onStatus,
  onTemplateRemoved,
  onTemplateInstalled,
}: TemplateMarketplaceProps): JSX.Element {
  const [templates, setTemplates] = useState<TemplateManifest[]>([]);
  const [loading, setLoading] = useState<boolean>(false);
  const [category, setCategory] = useState<TemplateCategory | "">("");
  const [query, setQuery] = useState<string>("");
  const [installPath, setInstallPath] = useState<string>("");
  const [installing, setInstalling] = useState<boolean>(false);
  const [pendingRemoveId, setPendingRemoveId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Pull the current installed list from the bridge. The search-box
  // value takes precedence over the category dropdown — see the
  // file-level comment for the rationale.
  const refresh = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const trimmed = query.trim();
      const report = await window.kcreate.templateMarketplace.list(
        trimmed === "" ? (category === "" ? undefined : category) : undefined,
        trimmed === "" ? undefined : trimmed,
      );
      setTemplates(report.templates);
    } catch (err) {
      const msg =
        err instanceof Error
          ? err.message
          : `template_list failed: ${String(err)}`;
      setErrorMsg(msg);
      onStatus?.(msg);
    } finally {
      setLoading(false);
    }
  }, [category, query, onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleCategoryChange = useCallback(
    (e: ChangeEvent<HTMLSelectElement>) => {
      const v = e.target.value;
      setCategory(v === "" ? "" : (v as TemplateCategory));
    },
    [],
  );

  const handleQueryChange = useCallback(
    (e: ChangeEvent<HTMLInputElement>) => {
      setQuery(e.target.value);
    },
    [],
  );

  const handleInstall = useCallback(async () => {
    const path = installPath.trim();
    if (path === "") {
      const msg = "template install: source path is empty";
      setErrorMsg(msg);
      onStatus?.(msg);
      return;
    }
    setInstalling(true);
    setErrorMsg(null);
    onStatus?.(`Installing template from ${path}…`);
    try {
      const manifest =
        await window.kcreate.templateMarketplace.installLocal(path);
      setInstallPath("");
      onStatus?.(`Installed "${manifest.name}" (v${manifest.version}).`);
      onTemplateInstalled?.(manifest);
      await refresh();
    } catch (err) {
      const msg =
        err instanceof Error
          ? err.message
          : `template_install_local failed: ${String(err)}`;
      setErrorMsg(msg);
      onStatus?.(msg);
    } finally {
      setInstalling(false);
    }
  }, [installPath, onStatus, onTemplateInstalled, refresh]);

  const handleRemoveConfirmed = useCallback(
    async (id: string) => {
      setErrorMsg(null);
      onStatus?.(null);
      try {
        await window.kcreate.templateMarketplace.remove(id);
        onTemplateRemoved?.(id);
        await refresh();
        onStatus?.("Template removed.");
      } catch (err) {
        const msg =
          err instanceof Error
            ? err.message
            : `template_remove failed: ${String(err)}`;
        setErrorMsg(msg);
        onStatus?.(msg);
      } finally {
        setPendingRemoveId(null);
      }
    },
    [onStatus, onTemplateRemoved, refresh],
  );

  // The category dropdown is informational while the search box is
  // non-empty: the bridge will ignore it. Surface that to the user
  // rather than leaving a confusing inert control.
  const categoryDisabled = query.trim() !== "";

  const totalCount = templates.length;
  const summaryText = useMemo(() => {
    if (loading) return "Loading templates…";
    if (totalCount === 0) {
      if (query.trim() !== "") {
        return `No templates match "${query.trim()}".`;
      }
      if (category !== "") {
        return `No templates in ${CATEGORY_LABELS[category]}.`;
      }
      return "No templates installed yet.";
    }
    return `${totalCount} template${totalCount === 1 ? "" : "s"}.`;
  }, [loading, totalCount, query, category]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        padding: spacing.md,
        fontFamily: font.family,
        color: colors.text,
        background: colors.bg,
      }}
    >
      <header
        style={{
          display: "flex",
          flexDirection: "column",
          gap: spacing.xs,
        }}
      >
        <h2
          style={{
            margin: 0,
            fontSize: 16,
            fontWeight: 600,
          }}
        >
          Template Marketplace
        </h2>
        <p
          style={{
            margin: 0,
            fontSize: 12,
            color: colors.textMuted,
          }}
        >
          Browse installed `.ktemplate` folders from{" "}
          <code>~/.kcreate/templates/</code>. Phase 3 is local-only — a
          future release will surface a hosted catalogue alongside
          local installs.
        </p>
      </header>

      <section
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: spacing.sm,
          alignItems: "center",
        }}
      >
        <input
          type="text"
          placeholder="Search by name, tag, or description"
          value={query}
          onChange={handleQueryChange}
          style={{
            flex: "1 1 220px",
            minWidth: 220,
            padding: "6px 10px",
            fontSize: 13,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            background: colors.bg,
            color: colors.text,
          }}
          aria-label="Search templates"
        />
        <select
          value={category}
          onChange={handleCategoryChange}
          disabled={categoryDisabled}
          style={{
            padding: "6px 10px",
            fontSize: 13,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            background: categoryDisabled ? colors.bgSoft : colors.bg,
            color: categoryDisabled ? colors.textMuted : colors.text,
            cursor: categoryDisabled ? "not-allowed" : "pointer",
          }}
          aria-label="Filter by category"
          title={
            categoryDisabled
              ? "Search overrides the category filter — clear the search box to filter by category."
              : undefined
          }
        >
          <option value="">All categories</option>
          {ALL_CATEGORIES.map((c) => (
            <option key={c} value={c}>
              {CATEGORY_LABELS[c]}
            </option>
          ))}
        </select>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={loading}
          style={{
            padding: "6px 12px",
            fontSize: 13,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            background: colors.bg,
            color: colors.text,
            cursor: loading ? "default" : "pointer",
          }}
        >
          Refresh
        </button>
      </section>

      <section
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: spacing.sm,
          alignItems: "center",
          padding: spacing.sm,
          border: `1px dashed ${colors.border}`,
          borderRadius: radius.md,
          background: colors.bgSoft,
        }}
      >
        <label
          htmlFor="template-install-path"
          style={{
            fontSize: 12,
            color: colors.textMuted,
          }}
        >
          Install from path:
        </label>
        <input
          id="template-install-path"
          type="text"
          placeholder="/path/to/template.ktemplate"
          value={installPath}
          onChange={(e) => setInstallPath(e.target.value)}
          style={{
            flex: "1 1 280px",
            minWidth: 280,
            padding: "6px 10px",
            fontSize: 13,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            background: colors.bg,
            color: colors.text,
          }}
          disabled={installing}
        />
        <button
          type="button"
          onClick={() => void handleInstall()}
          disabled={installing || installPath.trim() === ""}
          style={{
            padding: "6px 12px",
            fontSize: 13,
            border: "none",
            borderRadius: radius.sm,
            background:
              installing || installPath.trim() === ""
                ? colors.border
                : colors.accent,
            color: colors.textInverse,
            cursor:
              installing || installPath.trim() === "" ? "default" : "pointer",
          }}
        >
          {installing ? "Installing…" : "Install"}
        </button>
      </section>

      {errorMsg !== null ? (
        <div
          role="alert"
          style={{
            padding: spacing.sm,
            border: `1px solid ${colors.danger}`,
            borderRadius: radius.sm,
            background: colors.dangerBg,
            color: colors.danger,
            fontSize: 12,
          }}
        >
          {errorMsg}
        </div>
      ) : null}

      <div
        style={{
          fontSize: 12,
          color: colors.textMuted,
        }}
      >
        {summaryText}
      </div>

      <ul
        style={{
          listStyle: "none",
          margin: 0,
          padding: 0,
          display: "flex",
          flexDirection: "column",
          gap: spacing.sm,
        }}
      >
        {templates.map((t) => (
          <li
            key={t.id}
            style={{
              display: "flex",
              flexDirection: "column",
              gap: spacing.xs,
              padding: spacing.sm,
              border: `1px solid ${colors.border}`,
              borderRadius: radius.md,
              background: colors.bg,
            }}
          >
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: spacing.sm,
                justifyContent: "space-between",
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: spacing.sm,
                  flex: 1,
                  minWidth: 0,
                }}
              >
                <span
                  style={{
                    padding: "2px 8px",
                    borderRadius: radius.pill,
                    background: CATEGORY_TINT[t.category],
                    color: colors.textInverse,
                    fontSize: 11,
                    fontWeight: 500,
                    whiteSpace: "nowrap",
                  }}
                >
                  {CATEGORY_LABELS[t.category]}
                </span>
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
                  {t.name}
                </span>
                <span
                  style={{
                    fontSize: 11,
                    color: colors.textMuted,
                  }}
                >
                  v{t.version}
                </span>
              </div>
              {pendingRemoveId === t.id ? (
                <span
                  style={{
                    display: "flex",
                    gap: spacing.xs,
                  }}
                >
                  <button
                    type="button"
                    onClick={() => void handleRemoveConfirmed(t.id)}
                    style={{
                      padding: "4px 10px",
                      fontSize: 12,
                      border: "none",
                      borderRadius: radius.sm,
                      background: colors.danger,
                      color: colors.textInverse,
                      cursor: "pointer",
                    }}
                  >
                    Confirm remove
                  </button>
                  <button
                    type="button"
                    onClick={() => setPendingRemoveId(null)}
                    style={{
                      padding: "4px 10px",
                      fontSize: 12,
                      border: `1px solid ${colors.border}`,
                      borderRadius: radius.sm,
                      background: colors.bg,
                      color: colors.text,
                      cursor: "pointer",
                    }}
                  >
                    Cancel
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  onClick={() => setPendingRemoveId(t.id)}
                  style={{
                    padding: "4px 10px",
                    fontSize: 12,
                    border: `1px solid ${colors.danger}`,
                    borderRadius: radius.sm,
                    background: colors.bg,
                    color: colors.danger,
                    cursor: "pointer",
                  }}
                >
                  Remove
                </button>
              )}
            </div>
            {t.description !== "" ? (
              <p
                style={{
                  margin: 0,
                  fontSize: 12,
                  color: colors.textMuted,
                }}
              >
                {t.description}
              </p>
            ) : null}
            <div
              style={{
                display: "flex",
                flexWrap: "wrap",
                gap: spacing.xs,
                fontSize: 11,
                color: colors.textMuted,
              }}
            >
              <span>
                {t.page_count} page{t.page_count === 1 ? "" : "s"}
              </span>
              {t.author !== null ? <span>by {t.author}</span> : null}
              {t.tags.length > 0 ? (
                <span style={{ display: "flex", gap: spacing.xs }}>
                  {t.tags.map((tag) => (
                    <span
                      key={tag}
                      style={{
                        padding: "1px 6px",
                        background: colors.bgSoft,
                        color: colors.textMuted,
                        borderRadius: radius.sm,
                      }}
                    >
                      #{tag}
                    </span>
                  ))}
                </span>
              ) : null}
            </div>
            {t.source !== null && t.source.type === "local" ? (
              <code
                style={{
                  fontSize: 11,
                  color: colors.textMuted,
                  background: colors.bgSoft,
                  padding: "2px 6px",
                  borderRadius: radius.sm,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
                title={t.source.path}
              >
                {t.source.path}
              </code>
            ) : null}
          </li>
        ))}
      </ul>
    </div>
  );
}
