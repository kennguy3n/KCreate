// TemplateGallery tests — Workstream G2 (ready-made template library).
//
// Exercises the gallery's contract with the template-marketplace
// bridge end to end through the recording `kcreateStub`:
//   * the seeded catalog renders as thumbnail cards;
//   * the category chips re-query `templateMarketplace.list` with the
//     selected `TemplateCategory` and the grid follows the result;
//   * the (debounced) search box re-queries with the typed term and
//     the grid narrows;
//   * "Start from template" / "Duplicate & remix" delegate to the
//     `onStartFromTemplate` host callback with the selected id and the
//     correct `remix` flag.
//
// The stub's `list` override mirrors the real bridge filter/search
// contract (category dropped while a query is active — the search bar
// is the primary lens) so the test pins the *wiring*, not a
// re-implementation of the Rust-side filtering.

import { describe, it, expect, vi } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
} from "@testing-library/react";

import { TemplateGallery } from "./TemplateGallery";
import type {
  TemplateCategory,
  TemplateManifest,
} from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";

function manifest(
  id: string,
  name: string,
  category: TemplateCategory,
  tags: string[],
  description = "",
): TemplateManifest {
  return {
    id,
    name,
    description,
    category,
    tags,
    thumbnail: "thumbnail.png",
    page_count: 1,
    author: "KCreate",
    version: "1.0.0",
    source: { type: "local", path: `/tmp/${id}.ktemplate` },
  };
}

// A small but representative seeded catalog spanning two categories.
const CATALOG: TemplateManifest[] = [
  manifest("mobile-login", "Login — Welcome Back", "mobile_app", [
    "login",
    "auth",
  ]),
  manifest("mobile-feed", "Social Feed", "mobile_app", ["feed", "social"]),
  manifest("deck-title", "Pitch Deck — Title", "presentation", [
    "pitch",
    "title",
  ]),
  manifest("social-quote", "Quote Card", "social_media", ["quote"]),
];

/**
 * Install the recording stub with a `list` override that honours the
 * real bridge contract: when a query is present the category is
 * ignored and we match name/tag/description (case-insensitive);
 * otherwise we filter by category. Returns the spy so tests can assert
 * the exact `(category, query)` arguments the gallery passed.
 */
function stubCatalog(): void {
  const stub = kcreateStub();
  stub.override("templateMarketplace.list", (...args: unknown[]) => {
    const category = args[0] as TemplateCategory | undefined;
    const query = (args[1] as string | undefined)?.toLowerCase() ?? "";
    let out = CATALOG;
    if (query) {
      out = CATALOG.filter(
        (t) =>
          t.name.toLowerCase().includes(query) ||
          t.description.toLowerCase().includes(query) ||
          t.tags.some((tag) => tag.toLowerCase().includes(query)),
      );
    } else if (category) {
      out = CATALOG.filter((t) => t.category === category);
    }
    return { templates: out };
  });
}

function renderGallery() {
  const onStartFromTemplate = vi.fn(() => Promise.resolve());
  const onBack = vi.fn();
  const utils = render(
    <TemplateGallery
      onBack={onBack}
      onStartFromTemplate={onStartFromTemplate}
    />,
  );
  return { ...utils, onStartFromTemplate, onBack };
}

describe("TemplateGallery", () => {
  it("renders the seeded templates as cards", async () => {
    stubCatalog();
    renderGallery();
    for (const t of CATALOG) {
      await waitFor(() =>
        expect(
          screen.getByTestId(`kcreate-template-card-${t.id}`),
        ).toBeInTheDocument(),
      );
    }
  });

  it("narrows the grid when a category chip is selected", async () => {
    stubCatalog();
    renderGallery();
    // Wait for the full catalog first.
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-deck-title"),
      ).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByTestId("kcreate-template-cat-mobile_app"));

    await waitFor(() =>
      expect(
        screen.queryByTestId("kcreate-template-card-deck-title"),
      ).toBeNull(),
    );
    expect(
      screen.getByTestId("kcreate-template-card-mobile-login"),
    ).toBeInTheDocument();
    expect(
      screen.getByTestId("kcreate-template-card-mobile-feed"),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("kcreate-template-card-social-quote"),
    ).toBeNull();
  });

  it("narrows the grid by search query (name/tag/description)", async () => {
    stubCatalog();
    renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-feed"),
      ).toBeInTheDocument(),
    );

    // "feed" matches one card's name + tag; everything else drops.
    fireEvent.change(screen.getByTestId("kcreate-template-search"), {
      target: { value: "feed" },
    });

    await waitFor(() =>
      expect(
        screen.queryByTestId("kcreate-template-card-mobile-login"),
      ).toBeNull(),
    );
    expect(
      screen.getByTestId("kcreate-template-card-mobile-feed"),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("kcreate-template-card-deck-title"),
    ).toBeNull();
  });

  it("shows the empty state when a search matches nothing", async () => {
    stubCatalog();
    renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-login"),
      ).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByTestId("kcreate-template-search"), {
      target: { value: "zzzznomatch" },
    });

    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-empty"),
      ).toBeInTheDocument(),
    );
  });

  it("Start from template delegates to the host with remix=false", async () => {
    stubCatalog();
    const { onStartFromTemplate } = renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-feed"),
      ).toBeInTheDocument(),
    );

    // Select a specific card so the action targets a known id.
    fireEvent.click(screen.getByTestId("kcreate-template-card-mobile-feed"));
    await waitFor(() =>
      expect(screen.getByTestId("kcreate-template-start")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTestId("kcreate-template-start"));

    await waitFor(() =>
      expect(onStartFromTemplate).toHaveBeenCalledWith("mobile-feed", {
        remix: false,
      }),
    );
  });

  it("Duplicate & remix delegates to the host with remix=true", async () => {
    stubCatalog();
    const { onStartFromTemplate } = renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-login"),
      ).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByTestId("kcreate-template-card-mobile-login"));
    await waitFor(() =>
      expect(screen.getByTestId("kcreate-template-remix")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTestId("kcreate-template-remix"));

    await waitFor(() =>
      expect(onStartFromTemplate).toHaveBeenCalledWith("mobile-login", {
        remix: true,
      }),
    );
  });

  it("surfaces a bridge list() failure inline", async () => {
    const stub = kcreateStub();
    // The production bridge is async (IPC), so a failure surfaces as a
    // rejected promise, never a synchronous throw — model that here.
    stub.override("templateMarketplace.list", () =>
      Promise.reject(new Error("boom")),
    );
    renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-error"),
      ).toBeInTheDocument(),
    );
  });

  it("tallies category counts from an unfiltered catalog query", async () => {
    stubCatalog();
    renderGallery();
    // The "All" chip badge counts the whole catalog…
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-cat-all-count"),
      ).toHaveTextContent(String(CATALOG.length)),
    );
    // …and each category chip shows only its own slice (2 mobile-app).
    expect(
      screen.getByTestId("kcreate-template-cat-mobile_app-count"),
    ).toHaveTextContent("2");
  });

  it("Remix from file imports through the bridge and reloads", async () => {
    const stub = kcreateStub();
    let imported = false;
    const NEW = manifest("imported-remix", "Imported Remix", "custom", [
      "remix",
    ]);
    // The catalog gains the imported template only after import() runs —
    // mirroring the real bridge persisting it into the install dir.
    stub.override("templateMarketplace.list", (...args: unknown[]) => {
      const category = args[0] as TemplateCategory | undefined;
      const query = (args[1] as string | undefined)?.toLowerCase() ?? "";
      const base = imported ? [...CATALOG, NEW] : CATALOG;
      let out = base;
      if (query) {
        out = base.filter(
          (t) =>
            t.name.toLowerCase().includes(query) ||
            t.description.toLowerCase().includes(query) ||
            t.tags.some((tag) => tag.toLowerCase().includes(query)),
        );
      } else if (category) {
        out = base.filter((t) => t.category === category);
      }
      return { templates: out };
    });
    stub.override(
      "templateMarketplace.pickImport",
      () => "/tmp/design.kstudio",
    );
    stub.override("templateMarketplace.import", () => {
      imported = true;
      return NEW;
    });

    renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-login"),
      ).toBeInTheDocument(),
    );

    // The button opens a two-option menu (the OS dialog can't combine
    // file + directory picking on Windows/Linux); a `.kstudio` source
    // is a directory, so pick the "project or package" item.
    fireEvent.click(screen.getByTestId("kcreate-template-import"));
    fireEvent.click(screen.getByTestId("kcreate-template-import-package"));

    // The picker fires for the directory kind…
    await waitFor(() =>
      expect(
        stub.calls.some((c) => c.method === "templateMarketplace.pickImport"),
      ).toBe(true),
    );
    const pickCall = stub.calls.find(
      (c) => c.method === "templateMarketplace.pickImport",
    );
    expect(pickCall?.args[0]).toBe("directory");

    // …and the import bridge call fires with the chosen path.
    await waitFor(() =>
      expect(
        stub.calls.some((c) => c.method === "templateMarketplace.import"),
      ).toBe(true),
    );
    const importCall = stub.calls.find(
      (c) => c.method === "templateMarketplace.import",
    );
    expect(importCall?.args[0]).toEqual({ sourcePath: "/tmp/design.kstudio" });

    // …and the freshly imported template appears once the grid reloads.
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-imported-remix"),
      ).toBeInTheDocument(),
    );
  });

  it("does not call import when the file picker is cancelled", async () => {
    stubCatalog();
    const stub = kcreateStub();
    stub.override("templateMarketplace.pickImport", () => null);
    renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-login"),
      ).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByTestId("kcreate-template-import"));
    fireEvent.click(screen.getByTestId("kcreate-template-import-file"));

    await waitFor(() =>
      expect(
        stub.calls.some((c) => c.method === "templateMarketplace.pickImport"),
      ).toBe(true),
    );
    const pickCall = stub.calls.find(
      (c) => c.method === "templateMarketplace.pickImport",
    );
    expect(pickCall?.args[0]).toBe("file");
    expect(
      stub.calls.some((c) => c.method === "templateMarketplace.import"),
    ).toBe(false);
  });

  it("import menu offers a file picker and a package picker", async () => {
    stubCatalog();
    const stub = kcreateStub();
    renderGallery();
    await waitFor(() =>
      expect(
        screen.getByTestId("kcreate-template-card-mobile-login"),
      ).toBeInTheDocument(),
    );

    // Menu is closed until the button is clicked.
    expect(
      screen.queryByTestId("kcreate-template-import-menu"),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("kcreate-template-import"));
    expect(
      screen.getByTestId("kcreate-template-import-file"),
    ).toBeInTheDocument();
    expect(
      screen.getByTestId("kcreate-template-import-package"),
    ).toBeInTheDocument();

    // Clicking the backdrop dismisses the menu without importing.
    fireEvent.click(screen.getByTestId("kcreate-template-import-overlay"));
    await waitFor(() =>
      expect(
        screen.queryByTestId("kcreate-template-import-menu"),
      ).not.toBeInTheDocument(),
    );
    expect(
      stub.calls.some((c) => c.method === "templateMarketplace.pickImport"),
    ).toBe(false);
  });
});
