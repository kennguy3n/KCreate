// Shared `TemplateCategory` presentation metadata.
//
// `TemplateMarketplace.tsx`, `TemplatePicker.tsx`, and the G2
// `TemplateGallery.tsx` all need the same two things for every
// `TemplateCategory` discriminant: a human-readable label and an
// accent tint for the category badge / filter chip. These tables
// previously lived as byte-identical copies in each component and had
// already started drifting (the marketplace panel carried an
// `ALL_CATEGORIES` array the picker lacked). Centralising them here
// keeps the renderer in lockstep with the Rust enum
// (`kcreate_core::project::TemplateCategory`) from a single edit site:
// adding a category there is mirrored in exactly one place on the JS
// side.

import type { TemplateCategory } from "../../../shared/scene";

/// Human-readable labels for the snake_case `TemplateCategory`
/// discriminants. Kept in lockstep with the Rust enum — adding a
/// category there must be mirrored here AND in `CATEGORY_TINT`.
export const CATEGORY_LABELS: Record<TemplateCategory, string> = {
  pitch_deck: "Pitch Deck",
  proposal: "Proposal",
  brochure: "Brochure",
  flyer: "Flyer",
  report: "Report",
  presentation: "Presentation",
  social_media: "Social",
  mobile_app: "Mobile App",
  resume: "Resume",
  poster: "Poster",
  custom: "Custom",
};

/// Accent tint per category, used behind the category badge and as the
/// active-filter-chip background. Distinct hues so a dense gallery
/// reads at a glance.
export const CATEGORY_TINT: Record<TemplateCategory, string> = {
  pitch_deck: "#7E22CE",
  proposal: "#1D4ED8",
  brochure: "#0D9488",
  flyer: "#EA580C",
  report: "#374151",
  presentation: "#4338CA",
  social_media: "#DB2777",
  mobile_app: "#059669",
  resume: "#0891B2",
  poster: "#B45309",
  custom: "#4B5563",
};

/// Stable display order for category filters. Mirrors the order the
/// labels are declared above (most design-forward categories first,
/// `custom` last as the catch-all).
export const ALL_CATEGORIES: ReadonlyArray<TemplateCategory> = [
  "pitch_deck",
  "proposal",
  "brochure",
  "flyer",
  "report",
  "presentation",
  "social_media",
  "mobile_app",
  "resume",
  "poster",
  "custom",
];
