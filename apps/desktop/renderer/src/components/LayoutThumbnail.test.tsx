// LayoutThumbnail tests — the picker's real thumbnail surface.
//
// These pin the two things the "no thumbnail" fix depends on: the page
// pixel size is derived faithfully from page_size + orientation (so the
// viewBox matches the coordinate space the section bounds were authored
// in), and each section kind renders recognisable, non-blank content
// (real title type, an image gradient + glyph, a multi-bar chart, a
// page-count overlay) rather than an empty grey box.

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";

import {
  LayoutThumbnail,
  pagePixelSize,
  resolveAccent,
} from "./LayoutThumbnail";
import type {
  DesignTokens,
  TemplatePageDef,
  TemplateSectionDef,
} from "../../../shared/scene";

function section(
  kind: TemplateSectionDef["kind"],
  x: number,
  y: number,
  width: number,
  height: number,
  placeholder: string | null = null,
): TemplateSectionDef {
  return { kind, bounds: { x, y, width, height }, placeholder_text: placeholder };
}

// Mirrors the real "Pitch Deck" builtin: a 16:9 landscape slide whose
// sections are authored in absolute slide pixels (960×540).
const deckPage: TemplatePageDef = {
  name: "Title",
  page_size: { kind: "presentation_16x9" },
  orientation: "landscape",
  sections: [
    section("title", 60, 60, 840, 80, "The next chapter"),
    section("body_text", 60, 180, 840, 280, "Why now"),
    section("page_number", 880, 500, 60, 20),
  ],
};

describe("pagePixelSize", () => {
  it("derives a 16:9 landscape slide as 960×540 px", () => {
    expect(pagePixelSize(deckPage)).toEqual({ width: 960, height: 540 });
  });

  it("derives A4 portrait as 794×1123 px", () => {
    const a4: TemplatePageDef = {
      name: "Page",
      page_size: { kind: "a4" },
      orientation: "portrait",
      sections: [],
    };
    expect(pagePixelSize(a4)).toEqual({ width: 794, height: 1123 });
  });

  it("swaps width/height for landscape A4", () => {
    const a4l: TemplatePageDef = {
      name: "Page",
      page_size: { kind: "a4" },
      orientation: "landscape",
      sections: [],
    };
    expect(pagePixelSize(a4l)).toEqual({ width: 1123, height: 794 });
  });

  it("honours a custom page size", () => {
    const custom: TemplatePageDef = {
      name: "Page",
      page_size: { kind: "custom", width_mm: 100, height_mm: 50 },
      orientation: "portrait",
      sections: [],
    };
    const { width, height } = pagePixelSize(custom);
    // 100mm * 96/25.4 ≈ 378, 50mm ≈ 189.
    expect(width).toBe(378);
    expect(height).toBe(189);
  });
});

describe("resolveAccent", () => {
  it("falls back to the supplied accent when no tokens are present", () => {
    expect(resolveAccent("#7E22CE", null)).toBe("#7E22CE");
    expect(resolveAccent("#7E22CE", undefined)).toBe("#7E22CE");
  });

  it("prefers a primary design-token colour over the fallback", () => {
    const tokens: DesignTokens = {
      colors: { primary: { r: 1, g: 0, b: 0, a: 1 } },
      typography: {},
      spacing: {},
      radii: {},
      shadows: {},
    };
    expect(resolveAccent("#7E22CE", tokens)).toBe("rgba(255, 0, 0, 1)");
  });

  it("clamps out-of-range channels to the [0,1] wire contract", () => {
    // A channel slightly above 1 (e.g. float drift) must not flip the
    // colour to a 0–255 interpretation — it is clamped, matching the
    // renderer's other rgbaToCss helpers.
    const tokens: DesignTokens = {
      colors: { primary: { r: 1.0001, g: 0, b: 0, a: 1.5 } },
      typography: {},
      spacing: {},
      radii: {},
      shadows: {},
    };
    expect(resolveAccent("#7E22CE", tokens)).toBe("rgba(255, 0, 0, 1)");
  });
});

describe("LayoutThumbnail", () => {
  it("renders an SVG sized to the page's real pixel viewBox", () => {
    const { container } = render(
      <LayoutThumbnail page={deckPage} accent="#7E22CE" />,
    );
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("viewBox")).toBe("0 0 960 540");
  });

  it("renders real title placeholder text", () => {
    const { container } = render(
      <LayoutThumbnail page={deckPage} accent="#7E22CE" label="Pitch Deck" />,
    );
    const texts = Array.from(container.querySelectorAll("text")).map(
      (t) => t.textContent,
    );
    expect(texts).toContain("The next chapter");
  });

  it("renders an image section as a gradient block with a glyph", () => {
    const page: TemplatePageDef = {
      name: "Cover",
      page_size: { kind: "a4" },
      orientation: "landscape",
      sections: [section("image", 50, 50, 500, 380)],
    };
    const { container } = render(
      <LayoutThumbnail page={page} accent="#0D9488" />,
    );
    expect(container.querySelector("linearGradient")).not.toBeNull();
    // Picture glyph = a sun disc + two mountain polygons.
    expect(container.querySelector("circle")).not.toBeNull();
    expect(container.querySelectorAll("polygon").length).toBeGreaterThanOrEqual(
      2,
    );
  });

  it("renders a chart section as multiple bars", () => {
    const page: TemplatePageDef = {
      name: "Data",
      page_size: { kind: "a4" },
      orientation: "landscape",
      sections: [section("chart", 50, 50, 600, 300)],
    };
    const { container } = render(
      <LayoutThumbnail page={page} accent="#1D4ED8" />,
    );
    // Six bars (rects) + the surface background rect.
    const rects = container.querySelectorAll("rect");
    expect(rects.length).toBeGreaterThanOrEqual(6);
  });

  it("gives each instance unique gradient ids so simultaneously-mounted thumbnails don't collide", () => {
    const imagePage: TemplatePageDef = {
      name: "Cover",
      page_size: { kind: "a4" },
      orientation: "landscape",
      sections: [section("image", 50, 50, 500, 380)],
    };
    // Two cards with an image section at the same index render together in
    // the picker grid; their gradient ids must not clash (a clash makes the
    // second card paint with the first's accent via document-scoped url(#id)).
    const { container } = render(
      <div>
        <LayoutThumbnail page={imagePage} accent="#0D9488" />
        <LayoutThumbnail page={imagePage} accent="#DB2777" />
      </div>,
    );
    const ids = Array.from(container.querySelectorAll("linearGradient")).map(
      (g) => g.getAttribute("id") ?? "",
    );
    expect(ids.length).toBe(2);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) {
      expect(id).not.toContain(":");
    }
  });

  it("mixes an rgba() design-token accent correctly (no garbled hex parse)", () => {
    // When a template carries design tokens, resolveAccent returns an
    // rgba(...) string. The image-section gradient's light stop is mixed
    // from that accent toward white; parsing it as hex would garble the
    // colour (rgba(255,0,0,1) → a green tint). Assert the real ramp.
    const tokens: DesignTokens = {
      colors: { primary: { r: 1, g: 0, b: 0, a: 1 } },
      typography: {},
      spacing: {},
      radii: {},
      shadows: {},
    };
    const imagePage: TemplatePageDef = {
      name: "Cover",
      page_size: { kind: "a4" },
      orientation: "landscape",
      sections: [section("image", 50, 50, 500, 380)],
    };
    const { container } = render(
      <LayoutThumbnail page={imagePage} accent="#0D9488" tokens={tokens} />,
    );
    const stops = Array.from(container.querySelectorAll("stop")).map((s) =>
      s.getAttribute("stop-color"),
    );
    // First stop is the accent itself; second is the white-mixed light tint.
    expect(stops[0]).toBe("rgba(255, 0, 0, 1)");
    expect(stops[1]).toBe("rgb(255, 140, 140)");
  });

  it("renders overlay children (the page-count pill)", () => {
    const { getByText } = render(
      <LayoutThumbnail page={deckPage} accent="#7E22CE">
        <span>9 pages</span>
      </LayoutThumbnail>,
    );
    expect(getByText("9 pages")).toBeInTheDocument();
  });
});
