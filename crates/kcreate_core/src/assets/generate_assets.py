#!/usr/bin/env python3
# @generated-source for the KCreate built-in Elements / asset library.
#
# This script is the authoring tool for the bundled vector assets. It is
# NOT compiled into the crate (cargo ignores non-`.rs` files); it exists
# so the asset set is reproducible and its provenance is auditable —
# every glyph below is hand-authored geometry (no third-party artwork),
# so the whole library is license-clean and ships in-repo for offline use.
#
# Running it:
#     python3 crates/kcreate_core/src/assets/generate_assets.py
#
# It writes:
#   * one compact SVG per asset under `data/<category>/<id>.svg`
#   * `catalog.rs` — the `ASSET_DEFS` table consumed by `mod.rs`
#
# Conventions:
#   * 24x24 user-space grid (viewBox "0 0 24 24").
#   * Icons / frames: stroke style (`stroke`, no fill), 2px, round caps.
#   * Shapes / illustrations / badges: solid fills.
#   * Filled icons: a single solid fill (so theme recolour maps them
#     wholesale to the brand accent on insert).
# Every asset also carries a finer `group` (sub-category) so the panel
# can section a large catalogue (e.g. icons → Navigation / Weather /
# Charts). Colours are plain neutral defaults; inserted nodes are fully
# recolourable in the editor (and recoloured toward the active theme
# accent on insert — see `assets/recolor.rs`).

import math
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")

ICON_STROKE = "#1f2937"
SHAPE_FILL = "#4c6ef5"
FRAME_STROKE = "#1f2937"

# Illustration palette.
SKY = "#dbeafe"
SUN = "#fbbf24"
GRASS = "#34d399"
ROCK = "#64748b"
CORAL = "#fb7185"
INK = "#1f2937"
PAPER = "#f8fafc"
INDIGO = "#4c6ef5"
AMBER = "#f59e0b"
TEAL = "#14b8a6"
WHITE = "#ffffff"
LEAF = "#22c55e"
WOOD = "#92400e"
SLATE = "#475569"
BLUSH = "#f9a8d4"
VIOLET = "#8b5cf6"


def svg(body, view=24):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}" '
        f'width="{view}" height="{view}">{body}</svg>\n'
    )


def icon(body):
    """Wrap stroke-style icon markup in the shared presentation group."""
    return svg(
        f'<g fill="none" stroke="{ICON_STROKE}" stroke-width="2" '
        f'stroke-linecap="round" stroke-linejoin="round">{body}</g>'
    )


def filled(body):
    """Wrap a solid silhouette icon in a single-fill group (recolours
    wholesale to the theme accent on insert)."""
    return svg(f'<g fill="{ICON_STROKE}" stroke="none">{body}</g>')


def frame(body):
    return svg(
        f'<g fill="none" stroke="{FRAME_STROKE}" stroke-width="2" '
        f'stroke-linecap="round" stroke-linejoin="round">{body}</g>'
    )


def shape(body):
    return svg(body)


# --- Parametric geometry helpers (kept tiny + valid for usvg). ------------
def _p(x, y):
    return f"{x:.2f},{y:.2f}"


def poly_points(n, cx=12.0, cy=12.0, r=10.0, rot=-90.0):
    """`points` string for a regular n-gon."""
    return " ".join(
        _p(cx + r * math.cos(math.radians(rot + i * 360.0 / n)),
           cy + r * math.sin(math.radians(rot + i * 360.0 / n)))
        for i in range(n)
    )


def star_points(n, cx=12.0, cy=12.0, ro=10.0, ri=4.3, rot=-90.0):
    """`points` string for an n-pointed star (alternating outer/inner)."""
    pts = []
    for i in range(2 * n):
        r = ro if i % 2 == 0 else ri
        a = math.radians(rot + i * 180.0 / n)
        pts.append(_p(cx + r * math.cos(a), cy + r * math.sin(a)))
    return " ".join(pts)


# Each entry: (id, name, group, [tags], svg_text)
ASSETS = {"shapes": [], "lines": [], "icons": [], "frames": [], "illustrations": []}


def add(cat, _id, name, group, tags, svg_text):
    ASSETS[cat].append((_id, name, group, tags, svg_text))


# ==========================================================================
# SHAPES
# ==========================================================================
# -- Geometric -------------------------------------------------------------
add("shapes", "rectangle", "Rectangle", "Geometric", ["square", "box", "rect", "block"],
    shape(f'<rect x="2" y="5" width="20" height="14" fill="{SHAPE_FILL}"/>'))
add("shapes", "rounded-rectangle", "Rounded Rectangle", "Geometric", ["rounded", "box", "card", "rect"],
    shape(f'<rect x="2" y="5" width="20" height="14" rx="3" fill="{SHAPE_FILL}"/>'))
add("shapes", "square", "Square", "Geometric", ["box", "rect", "tile"],
    shape(f'<rect x="4" y="4" width="16" height="16" fill="{SHAPE_FILL}"/>'))
add("shapes", "rounded-square", "Rounded Square", "Geometric", ["box", "tile", "app"],
    shape(f'<rect x="4" y="4" width="16" height="16" rx="4" fill="{SHAPE_FILL}"/>'))
add("shapes", "circle", "Circle", "Geometric", ["round", "dot", "ellipse", "ball"],
    shape(f'<circle cx="12" cy="12" r="10" fill="{SHAPE_FILL}"/>'))
add("shapes", "ellipse", "Ellipse", "Geometric", ["oval", "round", "circle"],
    shape(f'<ellipse cx="12" cy="12" rx="11" ry="7" fill="{SHAPE_FILL}"/>'))
add("shapes", "oval-vertical", "Vertical Oval", "Geometric", ["oval", "ellipse", "round"],
    shape(f'<ellipse cx="12" cy="12" rx="7" ry="11" fill="{SHAPE_FILL}"/>'))
add("shapes", "pill", "Pill", "Geometric", ["capsule", "stadium", "tablet", "rounded"],
    shape(f'<rect x="2" y="8" width="20" height="8" rx="4" fill="{SHAPE_FILL}"/>'))
add("shapes", "triangle", "Triangle", "Geometric", ["delta", "pyramid", "polygon"],
    shape(f'<polygon points="12,3 22,21 2,21" fill="{SHAPE_FILL}"/>'))
add("shapes", "triangle-down", "Triangle Down", "Geometric", ["delta", "caret", "polygon"],
    shape(f'<polygon points="2,3 22,3 12,21" fill="{SHAPE_FILL}"/>'))
add("shapes", "right-triangle", "Right Triangle", "Geometric", ["corner", "ramp", "polygon"],
    shape(f'<polygon points="3,3 3,21 21,21" fill="{SHAPE_FILL}"/>'))
add("shapes", "diamond", "Diamond", "Geometric", ["rhombus", "gem", "kite"],
    shape(f'<polygon points="12,2 22,12 12,22 2,12" fill="{SHAPE_FILL}"/>'))
add("shapes", "kite", "Kite", "Geometric", ["rhombus", "polygon", "diamond"],
    shape(f'<polygon points="12,2 20,10 12,22 4,10" fill="{SHAPE_FILL}"/>'))
add("shapes", "pentagon", "Pentagon", "Geometric", ["polygon", "five"],
    shape(f'<polygon points="{poly_points(5)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "pentagon-down", "Inverted Pentagon", "Geometric", ["polygon", "five", "home"],
    shape(f'<polygon points="{poly_points(5, rot=90)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "hexagon", "Hexagon", "Geometric", ["polygon", "six", "honeycomb"],
    shape(f'<polygon points="6,3 18,3 23,12 18,21 6,21 1,12" fill="{SHAPE_FILL}"/>'))
add("shapes", "hexagon-flat", "Flat Hexagon", "Geometric", ["polygon", "six", "honeycomb"],
    shape(f'<polygon points="{poly_points(6, rot=0)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "heptagon", "Heptagon", "Geometric", ["polygon", "seven"],
    shape(f'<polygon points="{poly_points(7)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "octagon", "Octagon", "Geometric", ["polygon", "stop", "eight"],
    shape(f'<polygon points="8,2 16,2 22,8 22,16 16,22 8,22 2,16 2,8" fill="{SHAPE_FILL}"/>'))
add("shapes", "nonagon", "Nonagon", "Geometric", ["polygon", "nine"],
    shape(f'<polygon points="{poly_points(9)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "decagon", "Decagon", "Geometric", ["polygon", "ten"],
    shape(f'<polygon points="{poly_points(10)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "parallelogram", "Parallelogram", "Geometric", ["polygon", "slant", "skew"],
    shape(f'<polygon points="6,5 22,5 18,19 2,19" fill="{SHAPE_FILL}"/>'))
add("shapes", "trapezoid", "Trapezoid", "Geometric", ["polygon", "trapezium"],
    shape(f'<polygon points="6,5 18,5 22,19 2,19" fill="{SHAPE_FILL}"/>'))
add("shapes", "trapezoid-down", "Inverted Trapezoid", "Geometric", ["polygon", "trapezium", "funnel"],
    shape(f'<polygon points="2,5 22,5 18,19 6,19" fill="{SHAPE_FILL}"/>'))
add("shapes", "semicircle", "Semicircle", "Geometric", ["half", "round", "dome"],
    shape(f'<path d="M2 18 A10 10 0 0 1 22 18 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "quarter-circle", "Quarter Circle", "Geometric", ["quadrant", "pie", "corner"],
    shape(f'<path d="M3 21 V3 A18 18 0 0 1 21 21 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "arch", "Arch", "Geometric", ["door", "window", "dome"],
    shape(f'<path d="M4 21 V11 A8 8 0 0 1 20 11 V21 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "ring", "Ring", "Geometric", ["donut", "annulus", "loop", "round"],
    shape(f'<path d="M12 2 A10 10 0 1 1 11.99 2 Z M12 7 A5 5 0 1 0 12.01 7 Z" fill="{SHAPE_FILL}" fill-rule="evenodd"/>'))
add("shapes", "crescent", "Crescent", "Geometric", ["moon", "lune", "curve"],
    shape(f'<path d="M16 2 A10 10 0 1 0 16 22 A8 8 0 0 1 16 2 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "teardrop", "Teardrop", "Geometric", ["drop", "blob", "pin"],
    shape(f'<path d="M12 2 C18 9 20 13 20 15 A8 8 0 1 1 4 15 C4 13 6 9 12 2 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "blob", "Blob", "Geometric", ["organic", "splat", "abstract"],
    shape(f'<path d="M12 2 C17 2 22 5 22 11 C22 17 18 22 12 22 C7 22 2 18 2 12 C2 6 7 2 12 2 Z" fill="{SHAPE_FILL}"/>'))

# -- Stars -----------------------------------------------------------------
add("shapes", "star", "Star", "Stars", ["rating", "favorite", "five", "polygon"],
    shape(f'<polygon points="12,2 15,9 22.5,9.3 16.5,14 18.5,21.5 12,17.2 5.5,21.5 7.5,14 1.5,9.3 9,9" fill="{SHAPE_FILL}"/>'))
add("shapes", "star-four", "Four-Point Star", "Stars", ["sparkle", "polygon", "shine", "twinkle"],
    shape(f'<polygon points="{star_points(4, ri=3.4)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "star-six", "Six-Point Star", "Stars", ["sparkle", "polygon", "burst"],
    shape(f'<path d="M12 1 L15 7 L22 7 L17 12 L22 17 L15 17 L12 23 L9 17 L2 17 L7 12 L2 7 L9 7 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "star-eight", "Eight-Point Star", "Stars", ["sparkle", "polygon", "compass"],
    shape(f'<polygon points="{star_points(8, ri=5.2)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "star-twelve", "Twelve-Point Star", "Stars", ["sparkle", "polygon", "burst", "sun"],
    shape(f'<polygon points="{star_points(12, ri=7.2)}" fill="{SHAPE_FILL}"/>'))
add("shapes", "burst", "Burst", "Stars", ["explosion", "boom", "pow", "comic", "spike"],
    shape(f'<polygon points="{star_points(10, ro=11, ri=5)}" fill="{SHAPE_FILL}"/>'))

# -- Symbols ---------------------------------------------------------------
add("shapes", "speech-bubble", "Speech Bubble", "Symbols", ["chat", "message", "comment", "talk"],
    shape(f'<path d="M3 4 H21 A2 2 0 0 1 23 6 V15 A2 2 0 0 1 21 17 H10 L5 22 V17 H3 A2 2 0 0 1 1 15 V6 A2 2 0 0 1 3 4 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "speech-bubble-round", "Rounded Speech Bubble", "Symbols", ["chat", "message", "comment", "talk"],
    shape(f'<path d="M12 3 C5.4 3 1 7 1 11.5 C1 14 2.5 16.2 5 17.6 C4.7 19 3.8 20.4 2.5 21.5 C5 21.4 7.4 20.5 9.2 19.3 C10.1 19.5 11 19.6 12 19.6 C18.6 19.6 23 15.6 23 11.5 C23 7 18.6 3 12 3 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "heart", "Heart", "Symbols", ["love", "like", "favorite"],
    shape(f'<path d="M12 21 C12 21 2 14.5 2 8.2 C2 5 4.4 3 7.2 3 C9 3 10.6 4 12 5.8 C13.4 4 15 3 16.8 3 C19.6 3 22 5 22 8.2 C22 14.5 12 21 12 21 Z" fill="{CORAL}"/>'))
add("shapes", "plus-cross", "Cross", "Symbols", ["plus", "add", "medical"],
    shape(f'<path d="M9 2 H15 V9 H22 V15 H15 V22 H9 V15 H2 V9 H9 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "x-mark", "X Mark", "Symbols", ["close", "cancel", "cross", "no"],
    shape(f'<path d="M4 7 L7 4 L12 9 L17 4 L20 7 L15 12 L20 17 L17 20 L12 15 L7 20 L4 17 L9 12 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "shield", "Shield", "Symbols", ["security", "protect", "guard", "badge"],
    shape(f'<path d="M12 2 L21 5 V11 C21 16.5 17 20.5 12 22 C7 20.5 3 16.5 3 11 V5 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "lightning", "Lightning Bolt", "Symbols", ["bolt", "flash", "power", "energy", "zap"],
    shape(f'<polygon points="13,2 4,13 11,13 9,22 20,10 13,10" fill="{AMBER}"/>'))
add("shapes", "droplet", "Droplet", "Symbols", ["water", "drop", "liquid", "rain"],
    shape(f'<path d="M12 2 C12 2 5 11 5 15 A7 7 0 0 0 19 15 C19 11 12 2 12 2 Z" fill="{INDIGO}"/>'))
add("shapes", "gem", "Gem", "Symbols", ["diamond", "jewel", "crystal", "premium"],
    shape(f'<polygon points="6,3 18,3 23,9 12,22 1,9" fill="{TEAL}"/>'))
add("shapes", "cloud-shape", "Cloud", "Symbols", ["weather", "sky", "storage"],
    shape(f'<path d="M7 18 A4 4 0 0 1 7 10 A5 5 0 0 1 17 9 A3.5 3.5 0 0 1 17.5 18 Z" fill="{SKY}"/>'))
add("shapes", "location-pin", "Location Pin", "Symbols", ["map", "place", "marker", "gps"],
    shape(f'<path d="M12 2 A8 8 0 0 1 20 10 C20 16 12 23 12 23 C12 23 4 16 4 10 A8 8 0 0 1 12 2 Z" fill="{CORAL}"/>'))
add("shapes", "banner", "Banner", "Symbols", ["ribbon", "flag", "sale", "label"],
    shape(f'<polygon points="4,3 20,3 20,21 12,16 4,21" fill="{SHAPE_FILL}"/>'))

# -- Arrows (filled block arrows) ------------------------------------------
add("shapes", "arrow-block", "Block Arrow", "Arrows", ["arrow", "right", "direction", "next"],
    shape(f'<path d="M2 9 H13 V5 L22 12 L13 19 V15 H2 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "arrow-block-left", "Block Arrow Left", "Arrows", ["arrow", "left", "back", "previous"],
    shape(f'<path d="M22 9 H11 V5 L2 12 L11 19 V15 H22 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "arrow-block-up", "Block Arrow Up", "Arrows", ["arrow", "up", "top", "north"],
    shape(f'<path d="M9 22 V11 H5 L12 2 L19 11 H15 V22 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "arrow-block-down", "Block Arrow Down", "Arrows", ["arrow", "down", "bottom", "south"],
    shape(f'<path d="M9 2 V13 H5 L12 22 L19 13 H15 V2 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "arrow-block-double", "Double Block Arrow", "Arrows", ["arrow", "swap", "both", "exchange"],
    shape(f'<path d="M8 5 L8 9 H16 V5 L23 12 L16 19 V15 H8 V19 L1 12 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "chevron-block", "Chevron Block", "Arrows", ["arrow", "next", "forward", "ribbon"],
    shape(f'<polygon points="3,4 13,4 21,12 13,20 3,20 11,12" fill="{SHAPE_FILL}"/>'))

# -- Misc shapes -----------------------------------------------------------
add("shapes", "badge-seal", "Badge Seal", "Symbols", ["seal", "award", "burst", "starburst", "certified"],
    shape(f'<path d="M12 1 L14.3 4.1 L18 2.8 L18.2 6.7 L22 7.5 L19.9 10.8 L22.4 13.8 L18.8 15.4 L19 19.3 L15.2 18.4 L13.3 21.8 L12 18.2 L10.7 21.8 L8.8 18.4 L5 19.3 L5.2 15.4 L1.6 13.8 L4.1 10.8 L2 7.5 L5.8 6.7 L6 2.8 L9.7 4.1 Z" fill="{AMBER}"/>'))

# ==========================================================================
# LINES / ARROWS / CONNECTORS
# ==========================================================================
# -- Lines / dividers ------------------------------------------------------
add("lines", "line-horizontal", "Horizontal Line", "Lines", ["rule", "divider", "straight"],
    icon('<line x1="2" y1="12" x2="22" y2="12"/>'))
add("lines", "line-vertical", "Vertical Line", "Lines", ["rule", "divider", "straight"],
    icon('<line x1="12" y1="2" x2="12" y2="22"/>'))
add("lines", "line-diagonal", "Diagonal Line", "Lines", ["slash", "straight"],
    icon('<line x1="3" y1="21" x2="21" y2="3"/>'))
add("lines", "line-double", "Double Line", "Lines", ["rule", "divider", "parallel"],
    icon('<line x1="2" y1="9" x2="22" y2="9"/><line x1="2" y1="15" x2="22" y2="15"/>'))
add("lines", "line-dashed", "Dashed Line", "Lines", ["dash", "divider", "rule"],
    svg(f'<line x1="2" y1="12" x2="22" y2="12" fill="none" stroke="{ICON_STROKE}" stroke-width="2" stroke-linecap="round" stroke-dasharray="4 4"/>'))
add("lines", "line-dotted", "Dotted Line", "Lines", ["dots", "divider", "rule"],
    svg(f'<line x1="2" y1="12" x2="22" y2="12" fill="none" stroke="{ICON_STROKE}" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="0.1 4"/>'))
add("lines", "zigzag", "Zigzag", "Lines", ["wave", "line", "lightning"],
    icon('<polyline points="2,16 7,8 12,16 17,8 22,16"/>'))
add("lines", "wave-line", "Wave Line", "Lines", ["wave", "curve", "squiggle"],
    icon('<path d="M2 12 C5 6 8 6 11 12 C14 18 17 18 20 12"/>'))
add("lines", "wave-line-double", "Double Wave", "Lines", ["wave", "squiggle", "sea"],
    icon('<path d="M2 9 C5 4 8 4 11 9 C14 14 17 14 20 9"/><path d="M2 16 C5 11 8 11 11 16 C14 21 17 21 20 16"/>'))

# -- Arrows ----------------------------------------------------------------
add("lines", "arrow-right", "Arrow Right", "Arrows", ["next", "forward", "direction", "chevron"],
    icon('<line x1="3" y1="12" x2="20" y2="12"/><polyline points="14,6 20,12 14,18"/>'))
add("lines", "arrow-left", "Arrow Left", "Arrows", ["back", "previous", "direction", "chevron"],
    icon('<line x1="21" y1="12" x2="4" y2="12"/><polyline points="10,6 4,12 10,18"/>'))
add("lines", "arrow-up", "Arrow Up", "Arrows", ["top", "north", "direction", "chevron"],
    icon('<line x1="12" y1="21" x2="12" y2="4"/><polyline points="6,10 12,4 18,10"/>'))
add("lines", "arrow-down", "Arrow Down", "Arrows", ["bottom", "south", "direction", "chevron"],
    icon('<line x1="12" y1="3" x2="12" y2="20"/><polyline points="6,14 12,20 18,14"/>'))
add("lines", "arrow-up-right", "Arrow Up Right", "Arrows", ["diagonal", "northeast", "next", "direction"],
    icon('<line x1="5" y1="19" x2="19" y2="5"/><polyline points="9,5 19,5 19,15"/>'))
add("lines", "arrow-up-left", "Arrow Up Left", "Arrows", ["diagonal", "northwest", "back", "direction"],
    icon('<line x1="19" y1="19" x2="5" y2="5"/><polyline points="5,15 5,5 15,5"/>'))
add("lines", "arrow-down-right", "Arrow Down Right", "Arrows", ["diagonal", "southeast", "direction"],
    icon('<line x1="5" y1="5" x2="19" y2="19"/><polyline points="19,9 19,19 9,19"/>'))
add("lines", "arrow-down-left", "Arrow Down Left", "Arrows", ["diagonal", "southwest", "direction"],
    icon('<line x1="19" y1="5" x2="5" y2="19"/><polyline points="15,19 5,19 5,9"/>'))
add("lines", "arrow-double", "Double Arrow", "Arrows", ["both", "swap", "exchange", "next"],
    icon('<line x1="3" y1="12" x2="21" y2="12"/><polyline points="7,8 3,12 7,16"/><polyline points="17,8 21,12 17,16"/>'))
add("lines", "arrow-double-vertical", "Double Arrow Vertical", "Arrows", ["both", "swap", "resize"],
    icon('<line x1="12" y1="3" x2="12" y2="21"/><polyline points="8,7 12,3 16,7"/><polyline points="8,17 12,21 16,17"/>'))
add("lines", "arrow-curved", "Curved Arrow", "Arrows", ["bend", "redo", "turn", "next"],
    icon('<path d="M4 18 C4 10 9 6 19 6"/><polyline points="14,3 20,6 16,11"/>'))
add("lines", "arrow-elbow", "Elbow Arrow", "Arrows", ["connector", "corner", "turn", "right-angle", "next"],
    icon('<polyline points="4,5 4,16 18,16"/><polyline points="13,11 19,16 13,21"/>'))
add("lines", "arrow-return", "Return Arrow", "Arrows", ["enter", "back", "undo", "reply"],
    icon('<polyline points="20,5 20,12 6,12"/><polyline points="11,7 5,12 11,17"/>'))
add("lines", "arrow-bend-up", "Bend Up Arrow", "Arrows", ["turn", "redo", "forward"],
    icon('<polyline points="4,17 4,9 16,9"/><polyline points="11,4 17,9 11,14"/>'))
add("lines", "arrow-fork", "Fork Arrow", "Arrows", ["split", "branch", "diverge"],
    icon('<path d="M4 20 V12 C4 9 6 8 9 8 H18"/><path d="M4 12 C4 9 6 8 9 8 M18 8 L14 4 M18 8 L14 12"/>'))
add("lines", "arrow-merge", "Merge Arrow", "Arrows", ["join", "combine", "converge"],
    icon('<path d="M6 4 V10 C6 13 8 14 11 14 H20"/><path d="M18 4 V10 C18 13 16 14 13 14 M20 14 L16 10 M20 14 L16 18"/>'))
add("lines", "arrow-loop", "Loop Arrow", "Arrows", ["refresh", "repeat", "cycle", "redo"],
    icon('<path d="M5 7 A7 7 0 1 1 5 17"/><polyline points="2,4 5,7 8,4"/>'))
add("lines", "arrow-thin-right", "Thin Arrow", "Arrows", ["next", "minimal", "direction"],
    svg(f'<g fill="none" stroke="{ICON_STROKE}" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><line x1="3" y1="12" x2="21" y2="12"/><polyline points="15,7 21,12 15,17"/></g>'))

# -- Connectors ------------------------------------------------------------
add("lines", "connector-elbow", "Elbow Connector", "Connectors", ["connector", "flow", "link", "step"],
    icon('<polyline points="3,6 12,6 12,18 21,18"/>'))
add("lines", "connector-curved", "Curved Connector", "Connectors", ["connector", "flow", "link", "bezier"],
    icon('<path d="M3 6 C12 6 12 18 21 18"/>'))
add("lines", "connector-straight", "Straight Connector", "Connectors", ["connector", "link", "node", "edge"],
    icon('<circle cx="4" cy="6" r="2"/><circle cx="20" cy="18" r="2"/><line x1="5.6" y1="7.4" x2="18.4" y2="16.6"/>'))
add("lines", "connector-dashed", "Dashed Connector", "Connectors", ["connector", "flow", "link", "dotted"],
    svg(f'<g fill="none" stroke="{ICON_STROKE}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="4" cy="12" r="2"/><circle cx="20" cy="12" r="2"/><line x1="6" y1="12" x2="18" y2="12" stroke-dasharray="3 3"/></g>'))
add("lines", "connector-step", "Step Connector", "Connectors", ["connector", "flow", "stairs", "link"],
    icon('<polyline points="3,20 8,20 8,14 14,14 14,8 19,8 19,4"/>'))
add("lines", "connector-tree", "Tree Connector", "Connectors", ["branch", "hierarchy", "org", "split"],
    icon('<line x1="12" y1="3" x2="12" y2="9"/><path d="M5 15 V12 H19 V15"/><line x1="5" y1="15" x2="5" y2="18"/><line x1="12" y1="9" x2="12" y2="18"/><line x1="19" y1="15" x2="19" y2="18"/>'))

# -- Brackets / braces -----------------------------------------------------
add("lines", "bracket-left", "Left Bracket", "Brackets", ["bracket", "group", "scope"],
    icon('<path d="M9 3 H5 V21 H9"/>'))
add("lines", "bracket-right", "Right Bracket", "Brackets", ["bracket", "group", "scope"],
    icon('<path d="M15 3 H19 V21 H15"/>'))
add("lines", "brace-left", "Left Brace", "Brackets", ["curly", "group", "scope"],
    icon('<path d="M10 3 C7 3 8 11 5 12 C8 13 7 21 10 21"/>'))
add("lines", "brace-right", "Right Brace", "Brackets", ["curly", "group", "scope"],
    icon('<path d="M14 3 C17 3 16 11 19 12 C16 13 17 21 14 21"/>'))

# ==========================================================================
# ICONS
# ==========================================================================
# -- Navigation / UI -------------------------------------------------------
add("icons", "home", "Home", "Navigation", ["house", "main", "start", "dashboard"],
    icon('<path d="M3 11 L12 3 L21 11"/><path d="M5 9.5 V20 H19 V9.5"/><rect x="10" y="14" width="4" height="6"/>'))
add("icons", "menu", "Menu", "Navigation", ["hamburger", "list", "navigation", "bars"],
    icon('<line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/>'))
add("icons", "grid", "Grid", "Navigation", ["layout", "apps", "tiles", "dashboard"],
    icon('<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/>'))
add("icons", "chevron-up", "Chevron Up", "Navigation", ["arrow", "collapse", "up"],
    icon('<polyline points="5,15 12,8 19,15"/>'))
add("icons", "chevron-down", "Chevron Down", "Navigation", ["arrow", "expand", "down", "more"],
    icon('<polyline points="5,9 12,16 19,9"/>'))
add("icons", "chevron-left", "Chevron Left", "Navigation", ["arrow", "back", "previous"],
    icon('<polyline points="15,5 8,12 15,19"/>'))
add("icons", "chevron-right", "Chevron Right", "Navigation", ["arrow", "next", "forward"],
    icon('<polyline points="9,5 16,12 9,19"/>'))
add("icons", "chevrons-right", "Chevrons Right", "Navigation", ["arrow", "next", "forward", "fast"],
    icon('<polyline points="4,5 11,12 4,19"/><polyline points="13,5 20,12 13,19"/>'))
add("icons", "chevrons-left", "Chevrons Left", "Navigation", ["arrow", "back", "previous", "fast"],
    icon('<polyline points="20,5 13,12 20,19"/><polyline points="11,5 4,12 11,19"/>'))
add("icons", "more-horizontal", "More", "Navigation", ["dots", "menu", "ellipsis", "options"],
    icon('<circle cx="5" cy="12" r="1.4"/><circle cx="12" cy="12" r="1.4"/><circle cx="19" cy="12" r="1.4"/>'))
add("icons", "more-vertical", "More Vertical", "Navigation", ["dots", "menu", "ellipsis", "options"],
    icon('<circle cx="12" cy="5" r="1.4"/><circle cx="12" cy="12" r="1.4"/><circle cx="12" cy="19" r="1.4"/>'))
add("icons", "maximize", "Maximize", "Navigation", ["expand", "fullscreen", "corners"],
    icon('<polyline points="9,3 3,3 3,9"/><polyline points="15,3 21,3 21,9"/><polyline points="21,15 21,21 15,21"/><polyline points="3,15 3,21 9,21"/>'))
add("icons", "minimize", "Minimize", "Navigation", ["collapse", "shrink", "corners"],
    icon('<polyline points="3,9 9,9 9,3"/><polyline points="21,9 15,9 15,3"/><polyline points="15,21 15,15 21,15"/><polyline points="9,21 9,15 3,15"/>'))
add("icons", "external-link", "External Link", "Navigation", ["open", "new", "window", "out"],
    icon('<path d="M14 4 H20 V10"/><line x1="20" y1="4" x2="11" y2="13"/><path d="M18 13 V19 A1 1 0 0 1 17 20 H5 A1 1 0 0 1 4 19 V7 A1 1 0 0 1 5 6 H11"/>'))
add("icons", "sidebar", "Sidebar", "Navigation", ["layout", "panel", "menu"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><line x1="9" y1="4" x2="9" y2="20"/>'))
add("icons", "layout", "Layout", "Navigation", ["dashboard", "grid", "wireframe"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="10" y1="9" x2="10" y2="20"/>'))
add("icons", "columns", "Columns", "Navigation", ["layout", "split", "panels"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><line x1="12" y1="4" x2="12" y2="20"/>'))
add("icons", "move", "Move", "Navigation", ["drag", "arrows", "pan", "reposition"],
    icon('<line x1="12" y1="3" x2="12" y2="21"/><line x1="3" y1="12" x2="21" y2="12"/><polyline points="9,6 12,3 15,6"/><polyline points="9,18 12,21 15,18"/><polyline points="6,9 3,12 6,15"/><polyline points="18,9 21,12 18,15"/>'))
add("icons", "drag-handle", "Drag Handle", "Navigation", ["grip", "reorder", "move", "dots"],
    icon('<circle cx="9" cy="6" r="1.2"/><circle cx="15" cy="6" r="1.2"/><circle cx="9" cy="12" r="1.2"/><circle cx="15" cy="12" r="1.2"/><circle cx="9" cy="18" r="1.2"/><circle cx="15" cy="18" r="1.2"/>'))
add("icons", "zoom-in", "Zoom In", "Navigation", ["magnify", "search", "plus", "scale"],
    icon('<circle cx="11" cy="11" r="7"/><line x1="16" y1="16" x2="21" y2="21"/><line x1="11" y1="8" x2="11" y2="14"/><line x1="8" y1="11" x2="14" y2="11"/>'))
add("icons", "zoom-out", "Zoom Out", "Navigation", ["magnify", "search", "minus", "scale"],
    icon('<circle cx="11" cy="11" r="7"/><line x1="16" y1="16" x2="21" y2="21"/><line x1="8" y1="11" x2="14" y2="11"/>'))
add("icons", "log-in", "Log In", "Navigation", ["signin", "enter", "door", "access"],
    icon('<path d="M10 4 H6 A2 2 0 0 0 4 6 V18 A2 2 0 0 0 6 20 H10"/><polyline points="14,8 18,12 14,16"/><line x1="18" y1="12" x2="9" y2="12"/>'))
add("icons", "log-out", "Log Out", "Navigation", ["signout", "exit", "door", "leave"],
    icon('<path d="M14 4 H18 A2 2 0 0 1 20 6 V18 A2 2 0 0 1 18 20 H14"/><polyline points="9,8 5,12 9,16"/><line x1="5" y1="12" x2="16" y2="12"/>'))

# -- Status / alert --------------------------------------------------------
add("icons", "check", "Check", "Status", ["tick", "done", "ok", "success", "checkmark"],
    icon('<polyline points="4,12 10,18 20,6"/>'))
add("icons", "check-circle", "Check Circle", "Status", ["tick", "done", "ok", "success", "verified"],
    icon('<circle cx="12" cy="12" r="9"/><polyline points="8,12 11,15 16,9"/>'))
add("icons", "close", "Close", "Status", ["x", "cancel", "remove", "delete", "exit"],
    icon('<line x1="5" y1="5" x2="19" y2="19"/><line x1="19" y1="5" x2="5" y2="19"/>'))
add("icons", "x-circle", "Close Circle", "Status", ["x", "cancel", "error", "remove"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="9" y1="9" x2="15" y2="15"/><line x1="15" y1="9" x2="9" y2="15"/>'))
add("icons", "plus", "Plus", "Status", ["add", "new", "create", "more"],
    icon('<line x1="12" y1="4" x2="12" y2="20"/><line x1="4" y1="12" x2="20" y2="12"/>'))
add("icons", "plus-circle", "Plus Circle", "Status", ["add", "new", "create"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="12" y1="8" x2="12" y2="16"/><line x1="8" y1="12" x2="16" y2="12"/>'))
add("icons", "minus", "Minus", "Status", ["remove", "subtract", "less"],
    icon('<line x1="4" y1="12" x2="20" y2="12"/>'))
add("icons", "minus-circle", "Minus Circle", "Status", ["remove", "subtract", "delete"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="8" y1="12" x2="16" y2="12"/>'))
add("icons", "info", "Info", "Status", ["help", "about", "details", "information"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="12" y1="11" x2="12" y2="16"/><line x1="12" y1="8" x2="12" y2="8.1"/>'))
add("icons", "help-circle", "Help", "Status", ["question", "support", "faq"],
    icon('<circle cx="12" cy="12" r="9"/><path d="M9.2 9.5 A2.8 2.8 0 0 1 14.5 10.5 C14.5 12.5 12 12.5 12 15"/><line x1="12" y1="18" x2="12" y2="18.1"/>'))
add("icons", "alert-triangle", "Alert", "Status", ["warning", "caution", "danger", "exclaim"],
    icon('<path d="M12 3 L22 20 H2 Z"/><line x1="12" y1="9" x2="12" y2="14"/><line x1="12" y1="17" x2="12" y2="17.1"/>'))
add("icons", "alert-circle", "Alert Circle", "Status", ["warning", "error", "exclaim"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="12" y1="7" x2="12" y2="13"/><line x1="12" y1="16" x2="12" y2="16.1"/>'))
add("icons", "ban", "Ban", "Status", ["block", "forbidden", "no", "stop"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="5.6" y1="5.6" x2="18.4" y2="18.4"/>'))

# -- Media -----------------------------------------------------------------
add("icons", "play", "Play", "Media", ["start", "media", "video", "run"],
    icon('<polygon points="7,4 20,12 7,20" fill="none"/>'))
add("icons", "play-circle", "Play Circle", "Media", ["video", "media", "start", "watch"],
    icon('<circle cx="12" cy="12" r="9"/><polygon points="10,8.5 16,12 10,15.5"/>'))
add("icons", "pause", "Pause", "Media", ["stop", "media", "hold"],
    icon('<line x1="9" y1="5" x2="9" y2="19"/><line x1="15" y1="5" x2="15" y2="19"/>'))
add("icons", "stop", "Stop", "Media", ["square", "media", "end"],
    icon('<rect x="6" y="6" width="12" height="12" rx="1"/>'))
add("icons", "skip-forward", "Skip Forward", "Media", ["next", "media", "fast"],
    icon('<polygon points="5,5 14,12 5,19"/><line x1="19" y1="5" x2="19" y2="19"/>'))
add("icons", "skip-back", "Skip Back", "Media", ["previous", "media", "rewind"],
    icon('<polygon points="19,5 10,12 19,19"/><line x1="5" y1="5" x2="5" y2="19"/>'))
add("icons", "rewind", "Rewind", "Media", ["back", "media", "fast"],
    icon('<polygon points="11,5 3,12 11,19"/><polygon points="21,5 13,12 21,19"/>'))
add("icons", "fast-forward", "Fast Forward", "Media", ["next", "media", "speed"],
    icon('<polygon points="3,5 11,12 3,19"/><polygon points="13,5 21,12 13,19"/>'))
add("icons", "volume", "Volume", "Media", ["sound", "audio", "speaker", "loud"],
    icon('<polygon points="4,9 8,9 13,5 13,19 8,15 4,15"/><path d="M16 9 A4 4 0 0 1 16 15"/><path d="M18.5 6.5 A7 7 0 0 1 18.5 17.5"/>'))
add("icons", "volume-mute", "Mute", "Media", ["sound", "audio", "silent", "off"],
    icon('<polygon points="4,9 8,9 13,5 13,19 8,15 4,15"/><line x1="17" y1="9" x2="22" y2="15"/><line x1="22" y1="9" x2="17" y2="15"/>'))
add("icons", "mic", "Microphone", "Media", ["audio", "record", "voice", "sound"],
    icon('<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11 A7 7 0 0 0 19 11"/><line x1="12" y1="18" x2="12" y2="21"/><line x1="8" y1="21" x2="16" y2="21"/>'))
add("icons", "mic-off", "Mic Off", "Media", ["mute", "audio", "voice", "silent"],
    icon('<path d="M9 5 A3 3 0 0 1 15 6 V11"/><path d="M5 11 A7 7 0 0 0 16 16.5"/><line x1="12" y1="18" x2="12" y2="21"/><line x1="4" y1="4" x2="20" y2="20"/>'))
add("icons", "music", "Music", "Media", ["note", "song", "audio", "tune"],
    icon('<path d="M9 18 V5 L19 3 V16"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/>'))
add("icons", "headphones", "Headphones", "Media", ["audio", "music", "listen", "sound"],
    icon('<path d="M4 14 V12 A8 8 0 0 1 20 12 V14"/><rect x="3" y="14" width="4" height="6" rx="1.5"/><rect x="17" y="14" width="4" height="6" rx="1.5"/>'))
add("icons", "video", "Video", "Media", ["camera", "film", "record", "movie"],
    icon('<rect x="3" y="6" width="13" height="12" rx="2"/><polygon points="16,10 21,7 21,17 16,14"/>'))
add("icons", "film", "Film", "Media", ["movie", "video", "reel", "cinema"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><line x1="7" y1="4" x2="7" y2="20"/><line x1="17" y1="4" x2="17" y2="20"/><line x1="3" y1="12" x2="21" y2="12"/>'))
add("icons", "cast", "Cast", "Media", ["screen", "stream", "airplay", "tv"],
    icon('<path d="M3 7 V5 A1 1 0 0 1 4 4 H20 A1 1 0 0 1 21 5 V19 A1 1 0 0 1 20 20 H14"/><path d="M3 12 A6 6 0 0 1 9 18"/><path d="M3 16 A2 2 0 0 1 5 18"/><line x1="3" y1="20" x2="3.1" y2="20"/>'))

# -- Communication ---------------------------------------------------------
add("icons", "mail", "Mail", "Communication", ["email", "envelope", "message", "inbox"],
    icon('<rect x="3" y="5" width="18" height="14" rx="2"/><polyline points="3,7 12,13 21,7"/>'))
add("icons", "send", "Send", "Communication", ["paper-plane", "submit", "share", "message"],
    icon('<polygon points="3,11 21,3 13,21 11,13"/>'))
add("icons", "message-circle", "Chat", "Communication", ["message", "comment", "talk", "support", "bubble"],
    icon('<path d="M4 18 L4 7 A2 2 0 0 1 6 5 H18 A2 2 0 0 1 20 7 V15 A2 2 0 0 1 18 17 H8 Z"/>'))
add("icons", "message-square", "Message", "Communication", ["chat", "comment", "talk", "bubble"],
    icon('<path d="M4 16 V6 A2 2 0 0 1 6 4 H18 A2 2 0 0 1 20 6 V14 A2 2 0 0 1 18 16 H9 L5 20 Z"/>'))
add("icons", "phone", "Phone", "Communication", ["call", "contact", "telephone", "mobile"],
    icon('<path d="M5 3 H9 L11 8 L8.5 10.5 C9.5 12.5 11.5 14.5 13.5 15.5 L16 13 L21 15 V19 A2 2 0 0 1 19 21 C10 21 3 14 3 5 A2 2 0 0 1 5 3 Z"/>'))
add("icons", "phone-call", "Phone Call", "Communication", ["call", "ring", "contact"],
    icon('<path d="M5 3 H9 L11 8 L8.5 10.5 C9.5 12.5 11.5 14.5 13.5 15.5 L16 13 L21 15 V19 A2 2 0 0 1 19 21 C10 21 3 14 3 5 A2 2 0 0 1 5 3 Z"/><path d="M15 4 A5 5 0 0 1 20 9"/>'))
add("icons", "at-sign", "At Sign", "Communication", ["email", "mention", "username", "handle"],
    icon('<circle cx="12" cy="12" r="3.5"/><path d="M15.5 12 V13.5 A2.5 2.5 0 0 0 20 13 A9 9 0 1 0 16 20"/>'))
add("icons", "hash", "Hashtag", "Communication", ["pound", "tag", "number", "channel"],
    icon('<line x1="9" y1="3" x2="7" y2="21"/><line x1="17" y1="3" x2="15" y2="21"/><line x1="4" y1="9" x2="21" y2="9"/><line x1="3" y1="15" x2="20" y2="15"/>'))
add("icons", "inbox", "Inbox", "Communication", ["mail", "tray", "messages"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><polyline points="3,13 8,13 10,16 14,16 16,13 21,13"/>'))
add("icons", "reply", "Reply", "Communication", ["respond", "back", "answer", "arrow"],
    icon('<polyline points="9,7 4,12 9,17"/><path d="M4 12 H14 A6 6 0 0 1 20 18 V19"/>'))
add("icons", "rss", "RSS", "Communication", ["feed", "subscribe", "news", "blog"],
    icon('<path d="M5 13 A6 6 0 0 1 11 19"/><path d="M5 8 A11 11 0 0 1 16 19"/><circle cx="5.5" cy="18.5" r="1.3"/>'))
add("icons", "bell", "Bell", "Communication", ["notification", "alert", "alarm", "reminder"],
    icon('<path d="M6 16 V11 A6 6 0 0 1 18 11 V16 L20 18 H4 Z"/><path d="M10 18 A2 2 0 0 0 14 18"/>'))
add("icons", "bell-off", "Bell Off", "Communication", ["mute", "silent", "notification"],
    icon('<path d="M8 7 A6 6 0 0 1 18 11 V16 L20 18 H7"/><path d="M10 18 A2 2 0 0 0 14 18"/><line x1="4" y1="4" x2="20" y2="20"/>'))

# -- Files -----------------------------------------------------------------
add("icons", "folder", "Folder", "Files", ["directory", "files", "archive"],
    icon('<path d="M3 6 H9 L11 8 H21 V19 H3 Z"/>'))
add("icons", "folder-open", "Open Folder", "Files", ["directory", "files", "expand"],
    icon('<path d="M3 6 H9 L11 8 H19 V10 H6 L3 19 Z"/><path d="M3 19 L6 10 H23 L20 19 Z"/>'))
add("icons", "file", "File", "Files", ["document", "page", "paper"],
    icon('<path d="M6 3 H14 L19 8 V21 H6 Z"/><polyline points="14,3 14,8 19,8"/>'))
add("icons", "file-text", "Text File", "Files", ["document", "page", "lines", "report"],
    icon('<path d="M6 3 H14 L19 8 V21 H6 Z"/><polyline points="14,3 14,8 19,8"/><line x1="9" y1="12" x2="16" y2="12"/><line x1="9" y1="16" x2="16" y2="16"/>'))
add("icons", "file-plus", "Add File", "Files", ["document", "new", "create"],
    icon('<path d="M6 3 H14 L19 8 V21 H6 Z"/><polyline points="14,3 14,8 19,8"/><line x1="12" y1="11" x2="12" y2="17"/><line x1="9" y1="14" x2="15" y2="14"/>'))
add("icons", "document", "Document", "Files", ["file", "text", "page", "report"],
    icon('<path d="M6 3 H14 L19 8 V21 H6 Z"/><polyline points="14,3 14,8 19,8"/><line x1="9" y1="12" x2="16" y2="12"/><line x1="9" y1="16" x2="16" y2="16"/>'))
add("icons", "copy", "Copy", "Files", ["duplicate", "clone", "clipboard"],
    icon('<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8 V5 A1 1 0 0 0 15 4 H5 A1 1 0 0 0 4 5 V15 A1 1 0 0 0 5 16 H8"/>'))
add("icons", "clipboard", "Clipboard", "Files", ["paste", "copy", "tasks", "notes"],
    icon('<rect x="5" y="4" width="14" height="17" rx="2"/><rect x="9" y="2" width="6" height="4" rx="1"/>'))
add("icons", "paperclip", "Attachment", "Files", ["attach", "clip", "file"],
    icon('<path d="M19 11 L11 19 A4 4 0 0 1 5 13 L13 5 A2.7 2.7 0 0 1 17 9 L9.5 16.5 A1.3 1.3 0 0 1 7.5 14.5 L14 8"/>'))
add("icons", "save", "Save", "Files", ["disk", "store", "floppy"],
    icon('<path d="M4 4 H17 L20 7 V20 H4 Z"/><rect x="8" y="4" width="8" height="5"/><rect x="8" y="13" width="8" height="7"/>'))
add("icons", "printer", "Printer", "Files", ["print", "paper", "output"],
    icon('<path d="M7 9 V4 H17 V9"/><rect x="4" y="9" width="16" height="7" rx="1"/><rect x="7" y="14" width="10" height="6"/><line x1="16.5" y1="12" x2="16.5" y2="12.1"/>'))
add("icons", "archive", "Archive", "Files", ["box", "storage", "store", "zip"],
    icon('<rect x="3" y="4" width="18" height="4"/><path d="M5 8 V20 H19 V8"/><line x1="9.5" y1="12" x2="14.5" y2="12"/>'))
add("icons", "book", "Book", "Files", ["read", "library", "manual", "guide"],
    icon('<path d="M5 4 H17 A2 2 0 0 1 19 6 V20 H7 A2 2 0 0 1 5 18 Z"/><path d="M7 20 A2 2 0 0 1 5 18 V4"/>'))
add("icons", "book-open", "Open Book", "Files", ["read", "library", "manual", "story"],
    icon('<path d="M12 6 C10 4.5 6 4 4 4 V18 C6 18 10 18.5 12 20"/><path d="M12 6 C14 4.5 18 4 20 4 V18 C18 18 14 18.5 12 20"/>'))
add("icons", "newspaper", "Newspaper", "Files", ["news", "article", "press", "media"],
    icon('<path d="M4 5 H17 V20 H6 A2 2 0 0 1 4 18 Z"/><path d="M17 9 H20 V18 A2 2 0 0 1 18 20"/><line x1="7" y1="9" x2="14" y2="9"/><line x1="7" y1="13" x2="14" y2="13"/><line x1="7" y1="16" x2="11" y2="16"/>'))

# -- Editing / tools -------------------------------------------------------
add("icons", "edit", "Edit", "Editing", ["pencil", "write", "modify", "compose"],
    icon('<path d="M4 20 L4 16 L16 4 L20 8 L8 20 Z"/><line x1="13" y1="7" x2="17" y2="11"/>'))
add("icons", "pencil", "Pencil", "Editing", ["edit", "write", "draw"],
    icon('<path d="M14 4 L20 10 L9 21 L3 21 L3 15 Z"/><line x1="12" y1="6" x2="18" y2="12"/>'))
add("icons", "pen", "Pen", "Editing", ["write", "ink", "draw", "sign"],
    icon('<path d="M16 3 L21 8 L8 21 L3 21 L3 16 Z"/><path d="M14 5 L19 10"/>'))
add("icons", "brush", "Brush", "Editing", ["paint", "draw", "art", "color"],
    icon('<path d="M14 4 L20 10 L12 18 L8 14 Z"/><path d="M8 14 C5 15 4 17 3 21 C7 20 9 19 10 16"/>'))
add("icons", "eraser", "Eraser", "Editing", ["delete", "remove", "rubber", "clear"],
    icon('<path d="M8 21 L3 16 A2 2 0 0 1 3 13 L12 4 A2 2 0 0 1 15 4 L20 9 A2 2 0 0 1 20 12 L11 21 Z"/><line x1="8" y1="21" x2="20" y2="21"/>'))
add("icons", "crop", "Crop", "Editing", ["cut", "frame", "trim", "resize"],
    icon('<path d="M6 2 V16 A2 2 0 0 0 8 18 H22"/><path d="M2 6 H16 A2 2 0 0 1 18 8 V22"/>'))
add("icons", "type", "Text", "Editing", ["font", "typography", "letter", "type"],
    icon('<polyline points="5,7 5,5 19,5 19,7"/><line x1="12" y1="5" x2="12" y2="19"/><line x1="9" y1="19" x2="15" y2="19"/>'))
add("icons", "bold", "Bold", "Editing", ["text", "format", "strong", "weight"],
    icon('<path d="M7 4 H13 A4 4 0 0 1 13 12 H7 Z"/><path d="M7 12 H14 A4 4 0 0 1 14 20 H7 Z"/>'))
add("icons", "italic", "Italic", "Editing", ["text", "format", "slant", "oblique"],
    icon('<line x1="10" y1="4" x2="19" y2="4"/><line x1="5" y1="20" x2="14" y2="20"/><line x1="14" y1="4" x2="10" y2="20"/>'))
add("icons", "align-left", "Align Left", "Editing", ["text", "format", "paragraph"],
    icon('<line x1="4" y1="6" x2="20" y2="6"/><line x1="4" y1="11" x2="14" y2="11"/><line x1="4" y1="16" x2="18" y2="16"/>'))
add("icons", "align-center", "Align Center", "Editing", ["text", "format", "paragraph"],
    icon('<line x1="4" y1="6" x2="20" y2="6"/><line x1="7" y1="11" x2="17" y2="11"/><line x1="5" y1="16" x2="19" y2="16"/>'))
add("icons", "align-right", "Align Right", "Editing", ["text", "format", "paragraph"],
    icon('<line x1="4" y1="6" x2="20" y2="6"/><line x1="10" y1="11" x2="20" y2="11"/><line x1="6" y1="16" x2="20" y2="16"/>'))
add("icons", "list-bullet", "Bullet List", "Editing", ["list", "items", "ul", "points"],
    icon('<circle cx="5" cy="7" r="1.3"/><circle cx="5" cy="12" r="1.3"/><circle cx="5" cy="17" r="1.3"/><line x1="9" y1="7" x2="20" y2="7"/><line x1="9" y1="12" x2="20" y2="12"/><line x1="9" y1="17" x2="20" y2="17"/>'))
add("icons", "palette", "Palette", "Editing", ["color", "paint", "swatch", "art"],
    icon('<path d="M12 3 A9 9 0 1 0 12 21 C13.5 21 14 20 14 19 C14 17.5 13 17 14 16 C15 15 18 17 19.5 15 A8 8 0 0 0 12 3 Z"/><circle cx="8" cy="9" r="1.1"/><circle cx="12" cy="7" r="1.1"/><circle cx="16" cy="9" r="1.1"/>'))
add("icons", "eyedropper", "Eyedropper", "Editing", ["color", "pick", "sample", "dropper"],
    icon('<path d="M15 4 A2.5 2.5 0 0 1 19 8 L13 14 L10 11 Z"/><path d="M11 12 L4 19 A1.5 1.5 0 0 0 5 21 L12 14"/>'))
add("icons", "ruler", "Ruler", "Editing", ["measure", "scale", "design", "size"],
    icon('<rect x="2" y="8" width="20" height="8" rx="1" transform="rotate(45 12 12)"/><line x1="8" y1="8" x2="9.5" y2="9.5"/><line x1="11" y1="11" x2="12.5" y2="12.5"/><line x1="14" y1="14" x2="15.5" y2="15.5"/>'))
add("icons", "layers", "Layers", "Editing", ["stack", "levels", "z-index", "design"],
    icon('<polygon points="12,3 21,8 12,13 3,8"/><polyline points="3,12 12,17 21,12"/><polyline points="3,16 12,21 21,16"/>'))
add("icons", "magic-wand", "Magic Wand", "Editing", ["magic", "auto", "sparkle", "effects"],
    icon('<line x1="6" y1="18" x2="16" y2="8"/><path d="M18 4 L18.6 6 L20.5 6.6 L18.6 7.2 L18 9 L17.4 7.2 L15.5 6.6 L17.4 6 Z"/><line x1="13" y1="5" x2="13" y2="7"/><line x1="20" y1="12" x2="22" y2="12"/>'))
add("icons", "rotate", "Rotate", "Editing", ["refresh", "turn", "spin", "redo"],
    icon('<path d="M4 12 A8 8 0 1 1 6 17"/><polyline points="3,7 4,12 9,11"/>'))
add("icons", "flip-horizontal", "Flip Horizontal", "Editing", ["mirror", "reflect", "transform"],
    icon('<line x1="12" y1="3" x2="12" y2="21" stroke-dasharray="3 3"/><path d="M9 7 L4 12 L9 17 Z"/><path d="M15 7 L20 12 L15 17 Z"/>'))
add("icons", "group", "Group", "Editing", ["combine", "merge", "objects", "select"],
    icon('<rect x="4" y="4" width="10" height="10" rx="1"/><rect x="10" y="10" width="10" height="10" rx="1"/>'))

# -- Settings / controls ---------------------------------------------------
add("icons", "settings", "Settings", "Settings", ["gear", "cog", "preferences", "options"],
    icon('<circle cx="12" cy="12" r="3"/><path d="M12 2 L12 5 M12 19 L12 22 M2 12 L5 12 M19 12 L22 12 M4.9 4.9 L7 7 M17 17 L19.1 19.1 M19.1 4.9 L17 7 M7 17 L4.9 19.1"/>'))
add("icons", "sliders", "Sliders", "Settings", ["adjust", "controls", "filter", "settings"],
    icon('<line x1="4" y1="6" x2="20" y2="6"/><line x1="4" y1="12" x2="20" y2="12"/><line x1="4" y1="18" x2="20" y2="18"/><circle cx="9" cy="6" r="2"/><circle cx="15" cy="12" r="2"/><circle cx="8" cy="18" r="2"/>'))
add("icons", "filter", "Filter", "Settings", ["funnel", "sort", "refine"],
    icon('<polygon points="3,5 21,5 14,13 14,20 10,18 10,13"/>'))
add("icons", "toggle-on", "Toggle On", "Settings", ["switch", "enabled", "active"],
    icon('<rect x="2" y="7" width="20" height="10" rx="5"/><circle cx="16" cy="12" r="3"/>'))
add("icons", "toggle-off", "Toggle Off", "Settings", ["switch", "disabled", "inactive"],
    icon('<rect x="2" y="7" width="20" height="10" rx="5"/><circle cx="8" cy="12" r="3"/>'))
add("icons", "check-square", "Checkbox", "Settings", ["checked", "tick", "task", "done"],
    icon('<rect x="4" y="4" width="16" height="16" rx="2"/><polyline points="8,12 11,15 16,9"/>'))
add("icons", "radio", "Radio", "Settings", ["option", "select", "circle", "choice"],
    icon('<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="3.5"/>'))
add("icons", "power", "Power", "Settings", ["on", "off", "shutdown", "energy"],
    icon('<path d="M8 5.5 A8 8 0 1 0 16 5.5"/><line x1="12" y1="2" x2="12" y2="11"/>'))

# -- Search / view ---------------------------------------------------------
add("icons", "search", "Search", "View", ["find", "magnify", "look", "zoom"],
    icon('<circle cx="11" cy="11" r="7"/><line x1="16" y1="16" x2="21" y2="21"/>'))
add("icons", "eye", "Eye", "View", ["view", "visible", "show", "preview", "watch"],
    icon('<path d="M2 12 C5 6 9 4 12 4 C15 4 19 6 22 12 C19 18 15 20 12 20 C9 20 5 18 2 12 Z"/><circle cx="12" cy="12" r="3"/>'))
add("icons", "eye-off", "Eye Off", "View", ["hide", "hidden", "invisible", "private"],
    icon('<path d="M4 5 C2.8 6.7 2 9 2 12 C5 18 9 20 12 20 C13.7 20 15.6 19.3 17.3 18"/><path d="M9.5 5.2 C10.3 5.1 11.1 5 12 5 C15 5 19 7 22 13"/><line x1="3" y1="3" x2="21" y2="21"/>'))
add("icons", "image", "Image", "View", ["photo", "picture", "gallery", "media"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8.5" cy="9" r="1.8"/><polyline points="4,18 10,12 14,16 17,13 20,16"/>'))
add("icons", "camera", "Camera", "View", ["photo", "picture", "capture", "image"],
    icon('<path d="M3 7 H7 L9 5 H15 L17 7 H21 V19 H3 Z"/><circle cx="12" cy="13" r="3.5"/>'))

# -- Security --------------------------------------------------------------
add("icons", "lock", "Lock", "Security", ["secure", "private", "password", "locked"],
    icon('<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M8 10 V7 A4 4 0 0 1 16 7 V10"/>'))
add("icons", "unlock", "Unlock", "Security", ["secure", "open", "access", "unlocked"],
    icon('<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M8 10 V7 A4 4 0 0 1 15.5 5"/>'))
add("icons", "key", "Key", "Security", ["password", "access", "unlock", "login"],
    icon('<circle cx="8" cy="8" r="4.5"/><path d="M11 11 L20 20"/><line x1="17" y1="17" x2="19" y2="15"/><line x1="14" y1="14" x2="16" y2="12"/>'))
add("icons", "shield-check", "Shield Check", "Security", ["secure", "verified", "protect", "safe"],
    icon('<path d="M12 3 L20 6 V11 C20 16 16 19.5 12 21 C8 19.5 4 16 4 11 V6 Z"/><polyline points="9,12 11,14 15,9"/>'))
add("icons", "fingerprint", "Fingerprint", "Security", ["biometric", "touch", "id", "scan"],
    icon('<path d="M5 11 A7 7 0 0 1 19 11 V14"/><path d="M8.5 10.5 A4 4 0 0 1 15.5 11 V16"/><path d="M12 11 V18"/><path d="M8 16 V18"/>'))

# -- Commerce / finance ----------------------------------------------------
add("icons", "cart", "Shopping Cart", "Commerce", ["shop", "buy", "store", "ecommerce", "basket"],
    icon('<circle cx="9" cy="20" r="1.5"/><circle cx="18" cy="20" r="1.5"/><path d="M2 3 H5 L7 15 H19 L21 7 H6"/>'))
add("icons", "shopping-bag", "Shopping Bag", "Commerce", ["buy", "store", "purchase", "retail"],
    icon('<path d="M5 7 H19 L20 21 H4 Z"/><path d="M8 7 V6 A4 4 0 0 1 16 6 V7"/>'))
add("icons", "tag", "Tag", "Commerce", ["label", "price", "category", "sale"],
    icon('<path d="M3 11 V4 H10 L21 15 L14 22 Z"/><circle cx="7.5" cy="7.5" r="1.3"/>'))
add("icons", "credit-card", "Credit Card", "Commerce", ["payment", "pay", "bank", "money"],
    icon('<rect x="2" y="5" width="20" height="14" rx="2"/><line x1="2" y1="9.5" x2="22" y2="9.5"/><line x1="6" y1="14" x2="10" y2="14"/>'))
add("icons", "wallet", "Wallet", "Commerce", ["money", "cash", "purse", "bank"],
    icon('<path d="M3 7 A2 2 0 0 1 5 5 H17 V8"/><rect x="3" y="7" width="18" height="13" rx="2"/><circle cx="17" cy="13.5" r="1.4"/>'))
add("icons", "dollar-sign", "Dollar", "Commerce", ["money", "cash", "price", "currency"],
    icon('<line x1="12" y1="2" x2="12" y2="22"/><path d="M17 6.5 A4 4 0 0 0 13 4 H10.5 A3.5 3.5 0 0 0 10.5 11 H13.5 A3.5 3.5 0 0 1 13.5 18 H10 A4 4 0 0 1 6 15.5"/>'))
add("icons", "percent", "Percent", "Commerce", ["discount", "sale", "off", "rate"],
    icon('<line x1="19" y1="5" x2="5" y2="19"/><circle cx="7.5" cy="7.5" r="2.5"/><circle cx="16.5" cy="16.5" r="2.5"/>'))
add("icons", "receipt", "Receipt", "Commerce", ["bill", "invoice", "purchase", "order"],
    icon('<path d="M5 3 H19 V21 L16 19 L13 21 L10 19 L7 21 Z"/><line x1="9" y1="8" x2="15" y2="8"/><line x1="9" y1="12" x2="15" y2="12"/>'))
add("icons", "package", "Package", "Commerce", ["box", "shipping", "delivery", "parcel"],
    icon('<path d="M12 3 L21 7.5 V16.5 L12 21 L3 16.5 V7.5 Z"/><polyline points="3,7.5 12,12 21,7.5"/><line x1="12" y1="12" x2="12" y2="21"/>'))
add("icons", "truck", "Truck", "Commerce", ["delivery", "shipping", "transport", "logistics"],
    icon('<rect x="2" y="7" width="12" height="9"/><path d="M14 10 H18 L21 13 V16 H14 Z"/><circle cx="6.5" cy="18" r="1.6"/><circle cx="17.5" cy="18" r="1.6"/>'))
add("icons", "barcode", "Barcode", "Commerce", ["scan", "product", "upc", "store"],
    icon('<line x1="4" y1="6" x2="4" y2="18"/><line x1="7" y1="6" x2="7" y2="18"/><line x1="10" y1="6" x2="10" y2="18"/><line x1="14" y1="6" x2="14" y2="18"/><line x1="17" y1="6" x2="17" y2="18"/><line x1="20" y1="6" x2="20" y2="18"/>'))
add("icons", "coins", "Coins", "Commerce", ["money", "cash", "savings", "currency"],
    icon('<ellipse cx="9" cy="7" rx="6" ry="3"/><path d="M3 7 V11 C3 12.7 5.7 14 9 14 C12.3 14 15 12.7 15 11 V7"/><path d="M9 14 V18 C9 19.7 11.7 21 15 21 C18.3 21 21 19.7 21 18 V14 C21 12.3 18.3 11 15 11"/>'))
add("icons", "briefcase", "Briefcase", "Commerce", ["work", "business", "job", "office"],
    icon('<rect x="3" y="7" width="18" height="13" rx="2"/><path d="M9 7 V5 A1 1 0 0 1 10 4 H14 A1 1 0 0 1 15 5 V7"/><line x1="3" y1="12" x2="21" y2="12"/>'))
add("icons", "building", "Building", "Commerce", ["office", "company", "city", "work"],
    icon('<rect x="5" y="3" width="14" height="18"/><line x1="9" y1="7" x2="9" y2="7.1"/><line x1="15" y1="7" x2="15" y2="7.1"/><line x1="9" y1="11" x2="9" y2="11.1"/><line x1="15" y1="11" x2="15" y2="11.1"/><rect x="10" y="16" width="4" height="5"/>'))

# -- Charts / data ---------------------------------------------------------
add("icons", "chart-bar", "Bar Chart", "Charts", ["graph", "stats", "analytics", "data", "bars"],
    icon('<line x1="3" y1="21" x2="21" y2="21"/><rect x="5" y="11" width="3.5" height="9"/><rect x="10.2" y="6" width="3.5" height="14"/><rect x="15.5" y="14" width="3.5" height="6"/>'))
add("icons", "chart-line", "Line Chart", "Charts", ["graph", "stats", "analytics", "data", "trend"],
    icon('<polyline points="3,17 9,11 13,14 21,5"/><polyline points="16,5 21,5 21,10"/>'))
add("icons", "chart-pie", "Pie Chart", "Charts", ["graph", "stats", "analytics", "data", "donut"],
    icon('<path d="M12 3 A9 9 0 1 1 3 12 H12 Z"/><path d="M12 3 V12 H21 A9 9 0 0 0 12 3 Z"/>'))
add("icons", "chart-area", "Area Chart", "Charts", ["graph", "stats", "analytics", "data", "trend"],
    icon('<path d="M3 21 V14 L8 9 L13 13 L21 5 V21 Z"/>'))
add("icons", "trending-up", "Trending Up", "Charts", ["growth", "increase", "rise", "stats"],
    icon('<polyline points="3,17 9,11 13,15 21,7"/><polyline points="15,7 21,7 21,13"/>'))
add("icons", "trending-down", "Trending Down", "Charts", ["decline", "decrease", "fall", "loss"],
    icon('<polyline points="3,7 9,13 13,9 21,17"/><polyline points="15,17 21,17 21,11"/>'))
add("icons", "activity", "Activity", "Charts", ["pulse", "heartbeat", "monitor", "live"],
    icon('<polyline points="3,12 7,12 10,4 14,20 17,12 21,12"/>'))
add("icons", "gauge", "Gauge", "Charts", ["meter", "speed", "dial", "performance"],
    icon('<path d="M4 17 A9 9 0 1 1 20 17"/><line x1="12" y1="13" x2="16" y2="9"/><circle cx="12" cy="13" r="1.3"/>'))
add("icons", "scatter", "Scatter Plot", "Charts", ["graph", "stats", "data", "points"],
    icon('<line x1="4" y1="4" x2="4" y2="20"/><line x1="4" y1="20" x2="20" y2="20"/><circle cx="9" cy="14" r="1.2"/><circle cx="13" cy="9" r="1.2"/><circle cx="17" cy="11" r="1.2"/><circle cx="11" cy="16" r="1.2"/>'))
add("icons", "database", "Database", "Charts", ["data", "storage", "server", "sql"],
    icon('<ellipse cx="12" cy="6" rx="8" ry="3"/><path d="M4 6 V12 C4 13.7 7.6 15 12 15 C16.4 15 20 13.7 20 12 V6"/><path d="M4 12 V18 C4 19.7 7.6 21 12 21 C16.4 21 20 19.7 20 18 V12"/>'))

# -- Weather ---------------------------------------------------------------
add("icons", "sun", "Sun", "Weather", ["light", "day", "weather", "bright", "theme"],
    icon('<circle cx="12" cy="12" r="4.5"/><path d="M12 2 V4.5 M12 19.5 V22 M2 12 H4.5 M19.5 12 H22 M4.9 4.9 L6.7 6.7 M17.3 17.3 L19.1 19.1 M19.1 4.9 L17.3 6.7 M6.7 17.3 L4.9 19.1"/>'))
add("icons", "moon", "Moon", "Weather", ["night", "dark", "weather", "sleep", "theme"],
    icon('<path d="M20 14 A8 8 0 1 1 10 4 A6 6 0 0 0 20 14 Z"/>'))
add("icons", "cloud", "Cloud", "Weather", ["weather", "storage", "upload", "sky"],
    icon('<path d="M7 18 A4 4 0 0 1 7 10 A5 5 0 0 1 17 9 A3.5 3.5 0 0 1 17.5 18 Z"/>'))
add("icons", "cloud-rain", "Rain", "Weather", ["weather", "shower", "drizzle", "wet"],
    icon('<path d="M7 14 A4 4 0 0 1 7 6 A5 5 0 0 1 17 5 A3.5 3.5 0 0 1 17.5 14 Z"/><line x1="8" y1="17" x2="7" y2="20"/><line x1="12" y1="17" x2="11" y2="20"/><line x1="16" y1="17" x2="15" y2="20"/>'))
add("icons", "cloud-snow", "Snow", "Weather", ["weather", "winter", "cold", "flurry"],
    icon('<path d="M7 14 A4 4 0 0 1 7 6 A5 5 0 0 1 17 5 A3.5 3.5 0 0 1 17.5 14 Z"/><line x1="8" y1="18" x2="8" y2="18.1"/><line x1="12" y1="19" x2="12" y2="19.1"/><line x1="16" y1="18" x2="16" y2="18.1"/>'))
add("icons", "cloud-lightning", "Storm", "Weather", ["weather", "thunder", "bolt", "lightning"],
    icon('<path d="M7 13 A4 4 0 0 1 7 5 A5 5 0 0 1 17 4 A3.5 3.5 0 0 1 17.5 13 Z"/><polygon points="12,14 9,18 12,18 10,22 15,17 12,17"/>'))
add("icons", "umbrella", "Umbrella", "Weather", ["rain", "weather", "protect", "parasol"],
    icon('<path d="M3 12 A9 9 0 0 1 21 12 Z"/><path d="M12 12 V19 A2 2 0 0 0 16 19"/>'))
add("icons", "wind", "Wind", "Weather", ["breeze", "air", "weather", "blow"],
    icon('<path d="M3 8 H14 A2.5 2.5 0 1 0 11.5 5.5"/><path d="M3 12 H18 A2.5 2.5 0 1 1 15.5 14.5"/><path d="M3 16 H11 A2.5 2.5 0 1 1 8.5 18.5"/>'))
add("icons", "thermometer", "Thermometer", "Weather", ["temperature", "heat", "weather", "degrees"],
    icon('<path d="M12 3 A2.5 2.5 0 0 1 14.5 5.5 V14 A4 4 0 1 1 9.5 14 V5.5 A2.5 2.5 0 0 1 12 3 Z"/><line x1="12" y1="9" x2="12" y2="16"/>'))
add("icons", "snowflake", "Snowflake", "Weather", ["snow", "winter", "cold", "ice"],
    icon('<line x1="12" y1="2" x2="12" y2="22"/><line x1="3" y1="7" x2="21" y2="17"/><line x1="21" y1="7" x2="3" y2="17"/><polyline points="9,4 12,6 15,4"/><polyline points="9,20 12,18 15,20"/>'))
add("icons", "sunrise", "Sunrise", "Weather", ["dawn", "morning", "sun", "weather"],
    icon('<path d="M7 16 A5 5 0 0 1 17 16"/><line x1="2" y1="20" x2="22" y2="20"/><line x1="12" y1="2" x2="12" y2="6"/><line x1="5" y1="9" x2="6.5" y2="10.5"/><line x1="19" y1="9" x2="17.5" y2="10.5"/><polyline points="9,5 12,2 15,5"/>'))

# -- Devices ---------------------------------------------------------------
add("icons", "laptop", "Laptop", "Devices", ["computer", "device", "mac", "notebook"],
    icon('<rect x="4" y="5" width="16" height="11" rx="1"/><line x1="2" y1="20" x2="22" y2="20"/>'))
add("icons", "monitor", "Monitor", "Devices", ["screen", "display", "computer", "desktop"],
    icon('<rect x="3" y="4" width="18" height="12" rx="1"/><line x1="9" y1="20" x2="15" y2="20"/><line x1="12" y1="16" x2="12" y2="20"/>'))
add("icons", "smartphone", "Smartphone", "Devices", ["phone", "mobile", "device", "cell"],
    icon('<rect x="6" y="2" width="12" height="20" rx="2"/><line x1="10" y1="18" x2="14" y2="18"/>'))
add("icons", "tablet", "Tablet", "Devices", ["ipad", "device", "screen", "mobile"],
    icon('<rect x="4" y="3" width="16" height="18" rx="2"/><line x1="11" y1="18" x2="13" y2="18"/>'))
add("icons", "server", "Server", "Devices", ["data", "host", "rack", "cloud"],
    icon('<rect x="3" y="4" width="18" height="7" rx="1"/><rect x="3" y="13" width="18" height="7" rx="1"/><line x1="7" y1="7.5" x2="7.1" y2="7.5"/><line x1="7" y1="16.5" x2="7.1" y2="16.5"/>'))
add("icons", "cpu", "CPU", "Devices", ["chip", "processor", "tech", "hardware"],
    icon('<rect x="6" y="6" width="12" height="12" rx="1"/><rect x="9" y="9" width="6" height="6"/><line x1="9" y1="3" x2="9" y2="6"/><line x1="15" y1="3" x2="15" y2="6"/><line x1="9" y1="18" x2="9" y2="21"/><line x1="15" y1="18" x2="15" y2="21"/><line x1="3" y1="9" x2="6" y2="9"/><line x1="3" y1="15" x2="6" y2="15"/><line x1="18" y1="9" x2="21" y2="9"/><line x1="18" y1="15" x2="21" y2="15"/>'))
add("icons", "battery", "Battery", "Devices", ["power", "charge", "energy", "level"],
    icon('<rect x="2" y="8" width="18" height="9" rx="1.5"/><line x1="22" y1="11" x2="22" y2="14"/><rect x="4" y="10" width="9" height="5"/>'))
add("icons", "bluetooth", "Bluetooth", "Devices", ["wireless", "connect", "device", "pair"],
    icon('<polyline points="7,8 17,16 12,20 12,4 17,8 7,16"/>'))
add("icons", "keyboard", "Keyboard", "Devices", ["type", "input", "keys", "device"],
    icon('<rect x="2" y="6" width="20" height="12" rx="2"/><line x1="6" y1="10" x2="6" y2="10.1"/><line x1="10" y1="10" x2="10" y2="10.1"/><line x1="14" y1="10" x2="14" y2="10.1"/><line x1="18" y1="10" x2="18" y2="10.1"/><line x1="8" y1="14" x2="16" y2="14"/>'))
add("icons", "wifi", "Wi-Fi", "Devices", ["network", "signal", "internet", "wireless"],
    icon('<path d="M2 8.5 C8 3.5 16 3.5 22 8.5"/><path d="M5.5 12.5 C9 9.5 15 9.5 18.5 12.5"/><path d="M9 16.5 C10.7 15 13.3 15 15 16.5"/><line x1="12" y1="20" x2="12" y2="20.2"/>'))

# -- People / social -------------------------------------------------------
add("icons", "user", "User", "Social", ["person", "profile", "account", "avatar"],
    icon('<circle cx="12" cy="8" r="4"/><path d="M4 21 C4 16 8 14 12 14 C16 14 20 16 20 21"/>'))
add("icons", "users", "Users", "Social", ["people", "group", "team", "contacts"],
    icon('<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20 C2.5 15.5 6 13.5 9 13.5 C12 13.5 15.5 15.5 15.5 20"/><path d="M16 5 A3.5 3.5 0 0 1 16 12"/><path d="M17 13.7 C19.4 14.3 21.5 16.2 21.5 20"/>'))
add("icons", "user-plus", "Add User", "Social", ["person", "invite", "follow", "new"],
    icon('<circle cx="9" cy="8" r="4"/><path d="M2 21 C2 16 5.5 14 9 14 C11 14 12.8 14.6 14 15.8"/><line x1="18" y1="8" x2="18" y2="14"/><line x1="15" y1="11" x2="21" y2="11"/>'))
add("icons", "user-check", "User Verified", "Social", ["person", "approved", "confirm"],
    icon('<circle cx="9" cy="8" r="4"/><path d="M2 21 C2 16 5.5 14 9 14 C10.6 14 12 14.4 13.2 15.2"/><polyline points="15,13 17,15 21,11"/>'))
add("icons", "heart-outline", "Heart Outline", "Social", ["love", "like", "favorite"],
    icon('<path d="M12 20 C12 20 3 14 3 8.5 C3 6 5 4 7.5 4 C9.2 4 10.8 5 12 6.8 C13.2 5 14.8 4 16.5 4 C19 4 21 6 21 8.5 C21 14 12 20 12 20 Z"/>'))
add("icons", "star-outline", "Star Outline", "Social", ["rating", "favorite", "bookmark"],
    icon('<polygon points="12,3 14.6,8.6 21,9.2 16,13.6 17.6,20 12,16.5 6.4,20 8,13.6 3,9.2 9.4,8.6"/>'))
add("icons", "thumbs-up", "Thumbs Up", "Social", ["like", "approve", "vote", "good"],
    icon('<path d="M7 10 L11 3 C13 3 14 4.5 13.5 7 L13 10 H19 A2 2 0 0 1 21 12.4 L19.5 19 A2 2 0 0 1 17.5 20.5 H7 Z"/><line x1="7" y1="10" x2="7" y2="20.5"/><rect x="3" y="10" width="4" height="10.5"/>'))
add("icons", "thumbs-down", "Thumbs Down", "Social", ["dislike", "reject", "vote", "bad"],
    icon('<path d="M17 14 L13 21 C11 21 10 19.5 10.5 17 L11 14 H5 A2 2 0 0 1 3 11.6 L4.5 5 A2 2 0 0 1 6.5 3.5 H17 Z"/><line x1="17" y1="14" x2="17" y2="3.5"/><rect x="17" y="3.5" width="4" height="10.5"/>'))
add("icons", "share", "Share", "Social", ["send", "social", "network", "export"],
    icon('<circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><line x1="8.2" y1="10.8" x2="15.8" y2="7.2"/><line x1="8.2" y1="13.2" x2="15.8" y2="16.8"/>'))
add("icons", "smile", "Smile", "Social", ["happy", "emoji", "face", "good"],
    icon('<circle cx="12" cy="12" r="9"/><path d="M8 14 A4 4 0 0 0 16 14"/><line x1="9" y1="9.5" x2="9" y2="9.6"/><line x1="15" y1="9.5" x2="15" y2="9.6"/>'))
add("icons", "award", "Award", "Social", ["medal", "prize", "winner", "badge"],
    icon('<circle cx="12" cy="9" r="6"/><polyline points="9,14 7,22 12,19 17,22 15,14"/><polyline points="10,9 11.5,10.5 14.5,7.5"/>'))
add("icons", "crown", "Crown", "Social", ["king", "premium", "vip", "royal"],
    icon('<path d="M3 8 L7 13 L12 5 L17 13 L21 8 L19 20 H5 Z"/>'))

# -- Time ------------------------------------------------------------------
add("icons", "clock", "Clock", "Time", ["time", "watch", "schedule", "timer"],
    icon('<circle cx="12" cy="12" r="9"/><polyline points="12,7 12,12 16,14"/>'))
add("icons", "calendar", "Calendar", "Time", ["date", "schedule", "event", "month"],
    icon('<rect x="3" y="5" width="18" height="16" rx="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="8" y1="3" x2="8" y2="7"/><line x1="16" y1="3" x2="16" y2="7"/>'))
add("icons", "timer", "Timer", "Time", ["stopwatch", "countdown", "time", "clock"],
    icon('<circle cx="12" cy="13" r="8"/><line x1="12" y1="13" x2="12" y2="9"/><line x1="9" y1="2" x2="15" y2="2"/><line x1="12" y1="2" x2="12" y2="5"/>'))
add("icons", "hourglass", "Hourglass", "Time", ["time", "wait", "sand", "loading"],
    icon('<path d="M6 3 H18 V6 L13 12 L18 18 V21 H6 V18 L11 12 L6 6 Z"/>'))
add("icons", "alarm", "Alarm", "Time", ["clock", "wake", "reminder", "time"],
    icon('<circle cx="12" cy="13" r="7"/><polyline points="12,10 12,13 15,15"/><line x1="5" y1="3" x2="2" y2="6"/><line x1="19" y1="3" x2="22" y2="6"/>'))
add("icons", "history", "History", "Time", ["clock", "recent", "undo", "past"],
    icon('<path d="M4 12 A8 8 0 1 1 6 17"/><polyline points="3,7 4,12 9,11"/><polyline points="12,8 12,12 15,14"/>'))

# -- Nature ----------------------------------------------------------------
add("icons", "leaf", "Leaf", "Nature", ["plant", "eco", "nature", "green"],
    icon('<path d="M4 20 C4 10 11 4 20 4 C20 13 14 20 4 20 Z"/><path d="M4 20 C8 16 12 13 18 10"/>'))
add("icons", "flame", "Flame", "Nature", ["fire", "hot", "burn", "energy"],
    icon('<path d="M12 3 C14 7 18 9 18 14 A6 6 0 0 1 6 14 C6 11 8 10 9 8 C10 11 12 11 12 9 C12 7 11 5 12 3 Z"/>'))
add("icons", "feather", "Feather", "Nature", ["quill", "light", "write", "bird"],
    icon('<path d="M20 4 A6 6 0 0 0 11 4 L4 11 V20 H13 L20 13 A6 6 0 0 0 20 4 Z"/><line x1="16" y1="8" x2="6" y2="18"/><line x1="11" y1="9" x2="15" y2="13"/>'))
add("icons", "anchor", "Anchor", "Nature", ["ship", "boat", "marine", "nautical"],
    icon('<circle cx="12" cy="5" r="2.5"/><line x1="12" y1="7.5" x2="12" y2="21"/><path d="M5 13 A7 7 0 0 0 19 13"/><line x1="4" y1="13" x2="6" y2="13"/><line x1="18" y1="13" x2="20" y2="13"/>'))
add("icons", "compass", "Compass", "Nature", ["navigate", "direction", "explore", "map"],
    icon('<circle cx="12" cy="12" r="9"/><polygon points="16,8 11,11 8,16 13,13"/>'))
add("icons", "map-pin", "Map Pin", "Nature", ["location", "place", "marker", "gps", "pin"],
    icon('<path d="M12 22 C12 22 5 14.5 5 9 A7 7 0 0 1 19 9 C19 14.5 12 22 12 22 Z"/><circle cx="12" cy="9" r="2.5"/>'))
add("icons", "globe", "Globe", "Nature", ["world", "web", "internet", "language", "earth"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="3" y1="12" x2="21" y2="12"/><path d="M12 3 C15 6.5 15 17.5 12 21 C9 17.5 9 6.5 12 3 Z"/>'))
add("icons", "droplet-icon", "Droplet", "Nature", ["water", "rain", "liquid", "drop"],
    icon('<path d="M12 3 C12 3 6 10 6 14.5 A6 6 0 0 0 18 14.5 C18 10 12 3 12 3 Z"/>'))
add("icons", "tree", "Tree", "Nature", ["plant", "forest", "nature", "wood"],
    icon('<path d="M12 3 L7 11 H10 L6 17 H18 L14 11 H17 Z"/><line x1="12" y1="17" x2="12" y2="21"/>'))

# -- Travel ----------------------------------------------------------------
add("icons", "navigation", "Navigation", "Travel", ["direction", "compass", "arrow", "gps"],
    icon('<polygon points="12,3 20,21 12,16 4,21"/>'))
add("icons", "plane", "Plane", "Travel", ["flight", "travel", "airport", "fly"],
    icon('<path d="M2 14 L22 8 L20 12 L13 13 L9 21 L7 21 L8 13 L4 14 Z"/>'))
add("icons", "car", "Car", "Travel", ["vehicle", "drive", "auto", "transport"],
    icon('<path d="M3 14 L5 8 H19 L21 14 V18 H3 Z"/><circle cx="7" cy="18" r="1.6"/><circle cx="17" cy="18" r="1.6"/><line x1="3" y1="14" x2="21" y2="14"/>'))
add("icons", "bike", "Bike", "Travel", ["bicycle", "cycle", "ride", "transport"],
    icon('<circle cx="6" cy="17" r="3.5"/><circle cx="18" cy="17" r="3.5"/><path d="M6 17 L10 8 H15 L18 17"/><line x1="10" y1="8" x2="14" y2="8"/><line x1="13" y1="8" x2="11" y2="17"/>'))
add("icons", "bus", "Bus", "Travel", ["transit", "transport", "vehicle", "school"],
    icon('<rect x="4" y="4" width="16" height="13" rx="2"/><line x1="4" y1="11" x2="20" y2="11"/><circle cx="8" cy="20" r="1.4"/><circle cx="16" cy="20" r="1.4"/><line x1="8" y1="7" x2="8" y2="8"/><line x1="16" y1="7" x2="16" y2="8"/>'))
add("icons", "train", "Train", "Travel", ["transit", "rail", "metro", "transport"],
    icon('<rect x="6" y="3" width="12" height="14" rx="3"/><line x1="6" y1="10" x2="18" y2="10"/><line x1="9" y1="14" x2="9" y2="14.1"/><line x1="15" y1="14" x2="15" y2="14.1"/><line x1="8" y1="21" x2="6" y2="18"/><line x1="16" y1="21" x2="18" y2="18"/>'))
add("icons", "rocket", "Rocket", "Travel", ["launch", "startup", "boost", "space"],
    icon('<path d="M12 2 C16 5 17 10 16 15 H8 C7 10 8 5 12 2 Z"/><circle cx="12" cy="9" r="2"/><path d="M8 14 L5 17 L8 16 M16 14 L19 17 L16 16"/><path d="M10 16 L10 19 M14 16 L14 19"/>'))
add("icons", "fuel", "Fuel", "Travel", ["gas", "petrol", "station", "energy"],
    icon('<rect x="4" y="3" width="10" height="18" rx="1"/><line x1="4" y1="9" x2="14" y2="9"/><path d="M14 7 L17 7 A1 1 0 0 1 18 8 V15 A1.5 1.5 0 0 0 21 15 V10 L18 7"/>'))

# -- Misc ------------------------------------------------------------------
add("icons", "lightbulb", "Lightbulb", "Misc", ["idea", "tip", "innovation", "bright"],
    icon('<path d="M9 18 H15 M10 21 H14 M8 13 A6 6 0 1 1 16 13 C15 14 14.5 15 14.5 18 H9.5 C9.5 15 9 14 8 13 Z"/>'))
add("icons", "bookmark", "Bookmark", "Misc", ["save", "tag", "favorite", "ribbon"],
    icon('<path d="M6 3 H18 V21 L12 16 L6 21 Z"/>'))
add("icons", "link", "Link", "Misc", ["chain", "url", "connect", "hyperlink"],
    icon('<path d="M9 15 L15 9"/><path d="M11 6 L13 4 A4 4 0 0 1 18 9 L16 11"/><path d="M13 18 L11 20 A4 4 0 0 1 6 15 L8 13"/>'))
add("icons", "refresh", "Refresh", "Misc", ["reload", "sync", "update", "retry"],
    icon('<path d="M20 12 A8 8 0 1 1 17.5 6.5"/><polyline points="20,4 20,9 15,9"/>'))
add("icons", "download", "Download", "Misc", ["save", "import", "down", "arrow"],
    icon('<line x1="12" y1="3" x2="12" y2="15"/><polyline points="7,11 12,16 17,11"/><polyline points="4,20 20,20"/>'))
add("icons", "upload", "Upload", "Misc", ["export", "send", "up", "arrow"],
    icon('<line x1="12" y1="16" x2="12" y2="4"/><polyline points="7,9 12,4 17,9"/><polyline points="4,20 20,20"/>'))
add("icons", "trash", "Trash", "Misc", ["delete", "remove", "bin", "garbage"],
    icon('<polyline points="4,6 20,6"/><path d="M6 6 V20 H18 V6"/><path d="M9 6 V4 H15 V6"/><line x1="10" y1="10" x2="10" y2="17"/><line x1="14" y1="10" x2="14" y2="17"/>'))
add("icons", "gift", "Gift", "Misc", ["present", "reward", "box", "surprise"],
    icon('<rect x="4" y="9" width="16" height="12" rx="1"/><line x1="4" y1="13" x2="20" y2="13"/><line x1="12" y1="9" x2="12" y2="21"/><path d="M12 9 C9 9 7 5 9.5 5 C11.5 5 12 9 12 9 Z"/><path d="M12 9 C15 9 17 5 14.5 5 C12.5 5 12 9 12 9 Z"/>'))
add("icons", "target", "Target", "Misc", ["goal", "aim", "bullseye", "focus"],
    icon('<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><circle cx="12" cy="12" r="1.4"/>'))
add("icons", "trophy", "Trophy", "Misc", ["award", "win", "prize", "champion"],
    icon('<path d="M7 4 H17 V9 A5 5 0 0 1 7 9 Z"/><path d="M7 5 H4 V7 A3 3 0 0 0 7 10"/><path d="M17 5 H20 V7 A3 3 0 0 1 17 10"/><line x1="12" y1="14" x2="12" y2="17"/><path d="M8 20 H16 L15 17 H9 Z"/>'))
add("icons", "zap", "Zap", "Misc", ["lightning", "bolt", "flash", "energy"],
    icon('<polygon points="13,2 4,13 11,13 9,22 20,11 13,11"/>'))
add("icons", "magnet", "Magnet", "Misc", ["attract", "snap", "force", "pull"],
    icon('<path d="M5 4 H9 V13 A3 3 0 0 0 15 13 V4 H19 V13 A7 7 0 0 1 5 13 Z"/><line x1="5" y1="8" x2="9" y2="8"/><line x1="15" y1="8" x2="19" y2="8"/>'))
add("icons", "flag", "Flag", "Misc", ["marker", "report", "country", "milestone"],
    icon('<line x1="5" y1="3" x2="5" y2="21"/><path d="M5 4 H18 L15 8 L18 12 H5 Z"/>'))
add("icons", "puzzle", "Puzzle", "Misc", ["piece", "plugin", "extension", "solve"],
    icon('<path d="M9 4 A2 2 0 0 1 13 4 V6 H17 V10 A2 2 0 0 1 17 14 H17 V18 H13 A2 2 0 0 0 9 18 H5 V14 A2 2 0 0 0 5 10 V6 H9 Z"/>'))

# -- Filled icon variants (single solid fill → recolour wholesale) ---------
add("icons", "heart-filled", "Heart Filled", "Filled", ["love", "like", "favorite", "solid"],
    filled('<path d="M12 21 C12 21 2 14.5 2 8.2 C2 5 4.4 3 7.2 3 C9 3 10.6 4 12 5.8 C13.4 4 15 3 16.8 3 C19.6 3 22 5 22 8.2 C22 14.5 12 21 12 21 Z"/>'))
add("icons", "star-filled", "Star Filled", "Filled", ["rating", "favorite", "solid"],
    filled('<polygon points="12,2 15,9 22.5,9.3 16.5,14 18.5,21.5 12,17.2 5.5,21.5 7.5,14 1.5,9.3 9,9"/>'))
add("icons", "bell-filled", "Bell Filled", "Filled", ["notification", "alert", "solid"],
    filled('<path d="M6 16 V11 A6 6 0 0 1 18 11 V16 L20 18 H4 Z"/><path d="M9.5 19 A2.5 2.5 0 0 0 14.5 19 Z"/>'))
add("icons", "bookmark-filled", "Bookmark Filled", "Filled", ["save", "favorite", "solid"],
    filled('<path d="M6 3 H18 V21 L12 16 L6 21 Z"/>'))
add("icons", "circle-filled", "Circle Filled", "Filled", ["dot", "round", "solid"],
    filled('<circle cx="12" cy="12" r="9"/>'))
add("icons", "square-filled", "Square Filled", "Filled", ["box", "stop", "solid"],
    filled('<rect x="4" y="4" width="16" height="16" rx="2"/>'))
add("icons", "play-filled", "Play Filled", "Filled", ["start", "media", "solid"],
    filled('<polygon points="7,4 20,12 7,20"/>'))
add("icons", "check-circle-filled", "Check Circle Filled", "Filled", ["done", "success", "solid"],
    filled('<path d="M12 3 A9 9 0 1 0 12 21 A9 9 0 0 0 12 3 Z M11 16 L7 12 L8.4 10.6 L11 13.2 L15.6 8.6 L17 10 Z" fill-rule="evenodd"/>'))
add("icons", "user-filled", "User Filled", "Filled", ["person", "profile", "avatar", "solid"],
    filled('<circle cx="12" cy="8" r="4"/><path d="M4 21 C4 16 8 14 12 14 C16 14 20 16 20 21 Z"/>'))
add("icons", "home-filled", "Home Filled", "Filled", ["house", "main", "solid"],
    filled('<path d="M12 3 L22 12 H19 V21 H14 V15 H10 V21 H5 V12 H2 Z"/>'))
add("icons", "sun-filled", "Sun Filled", "Filled", ["light", "day", "bright", "solid"],
    filled('<circle cx="12" cy="12" r="5"/><path d="M11 1 H13 V4 H11 Z M11 20 H13 V23 H11 Z M1 11 H4 V13 H1 Z M20 11 H23 V13 H20 Z"/>'))
add("icons", "moon-filled", "Moon Filled", "Filled", ["night", "dark", "sleep", "solid"],
    filled('<path d="M20 14 A8 8 0 1 1 10 4 A6 6 0 0 0 20 14 Z"/>'))
add("icons", "cloud-filled", "Cloud Filled", "Filled", ["weather", "sky", "solid"],
    filled('<path d="M7 19 A4.5 4.5 0 0 1 7 10 A5.5 5.5 0 0 1 17.5 9 A4 4 0 0 1 18 19 Z"/>'))
add("icons", "tag-filled", "Tag Filled", "Filled", ["label", "price", "solid"],
    filled('<path d="M3 11 V4 H10 L21 15 L14 22 Z M7.5 6 A1.5 1.5 0 1 0 7.51 6 Z" fill-rule="evenodd"/>'))
add("icons", "message-filled", "Message Filled", "Filled", ["chat", "comment", "solid"],
    filled('<path d="M4 4 H20 A2 2 0 0 1 22 6 V15 A2 2 0 0 1 20 17 H9 L4 21 Z"/>'))
add("icons", "lock-filled", "Lock Filled", "Filled", ["secure", "private", "solid"],
    filled('<path d="M8 9 V7 A4 4 0 0 1 16 7 V9 H17 A1 1 0 0 1 18 10 V20 A1 1 0 0 1 17 21 H7 A1 1 0 0 1 6 20 V10 A1 1 0 0 1 7 9 Z M10 9 H14 V7 A2 2 0 0 0 10 7 Z" fill-rule="evenodd"/>'))
add("icons", "camera-filled", "Camera Filled", "Filled", ["photo", "capture", "solid"],
    filled('<path d="M9 4 H15 L17 6 H21 A1 1 0 0 1 22 7 V19 A1 1 0 0 1 21 20 H3 A1 1 0 0 1 2 19 V7 A1 1 0 0 1 3 6 H7 Z M12 9 A4 4 0 1 0 12 17 A4 4 0 0 0 12 9 Z" fill-rule="evenodd"/>'))
add("icons", "mail-filled", "Mail Filled", "Filled", ["email", "envelope", "solid"],
    filled('<path d="M3 6 H21 A1 1 0 0 1 22 7 L12 14 L2 7 A1 1 0 0 1 3 6 Z"/><path d="M2 9 L12 16 L22 9 V18 A1 1 0 0 1 21 19 H3 A1 1 0 0 1 2 18 Z"/>'))
add("icons", "flag-filled", "Flag Filled", "Filled", ["marker", "milestone", "solid"],
    filled('<rect x="4" y="3" width="2" height="18" rx="1"/><path d="M6 4 H19 L16 8 L19 12 H6 Z"/>'))
add("icons", "thumbs-up-filled", "Thumbs Up Filled", "Filled", ["like", "approve", "solid"],
    filled('<path d="M7 10 L11 3 C13.2 3 14.2 4.6 13.6 7.2 L13 10 H19.5 A2 2 0 0 1 21.4 12.5 L19.9 19 A2 2 0 0 1 18 20.5 H7 Z"/><rect x="2" y="10" width="4" height="10.5" rx="1"/>'))
add("icons", "pin-filled", "Pin Filled", "Filled", ["location", "place", "marker", "solid"],
    filled('<path d="M12 2 A7 7 0 0 1 19 9 C19 15 12 22 12 22 C12 22 5 15 5 9 A7 7 0 0 1 12 2 Z M12 6.5 A2.5 2.5 0 1 0 12 11.5 A2.5 2.5 0 0 0 12 6.5 Z" fill-rule="evenodd"/>'))
add("icons", "shield-filled", "Shield Filled", "Filled", ["secure", "protect", "solid"],
    filled('<path d="M12 2 L20 5 V11 C20 16 16 19.5 12 21 C8 19.5 4 16 4 11 V5 Z"/>'))
add("icons", "info-filled", "Info Filled", "Filled", ["help", "about", "solid"],
    filled('<path d="M12 3 A9 9 0 1 0 12 21 A9 9 0 0 0 12 3 Z M11 10 H13 V17 H11 Z M11 6.5 H13 V8.5 H11 Z" fill-rule="evenodd"/>'))
add("icons", "alert-filled", "Alert Filled", "Filled", ["warning", "caution", "solid"],
    filled('<path d="M12 3 L22 20 H2 Z M11 9 H13 V14 H11 Z M11 16 H13 V18 H11 Z" fill-rule="evenodd"/>'))
add("icons", "eye-filled", "Eye Filled", "Filled", ["view", "visible", "show", "solid"],
    filled('<path d="M2 12 C5 6 9 4 12 4 C15 4 19 6 22 12 C19 18 15 20 12 20 C9 20 5 18 2 12 Z M12 8 A4 4 0 1 0 12 16 A4 4 0 0 0 12 8 Z" fill-rule="evenodd"/>'))
add("icons", "fire-filled", "Fire Filled", "Filled", ["flame", "hot", "trending", "solid"],
    filled('<path d="M12 2 C14 6 18 8 18 14 A6 6 0 0 1 6 14 C6 11 8 10 9 7 C10 10 12 10 12 8 C12 6 11 4 12 2 Z"/>'))

# ==========================================================================
# FRAMES / DIVIDERS / BADGES
# ==========================================================================
# -- Frames ----------------------------------------------------------------
add("frames", "frame-rectangle", "Rectangle Frame", "Frames", ["border", "outline", "box"],
    frame('<rect x="2" y="4" width="20" height="16"/>'))
add("frames", "frame-rounded", "Rounded Frame", "Frames", ["border", "outline", "card"],
    frame('<rect x="2" y="4" width="20" height="16" rx="3"/>'))
add("frames", "frame-circle", "Circle Frame", "Frames", ["border", "outline", "round", "avatar"],
    frame('<circle cx="12" cy="12" r="10"/>'))
add("frames", "frame-square", "Square Frame", "Frames", ["border", "outline", "box"],
    frame('<rect x="4" y="4" width="16" height="16"/>'))
add("frames", "frame-oval", "Oval Frame", "Frames", ["border", "outline", "ellipse"],
    frame('<ellipse cx="12" cy="12" rx="10" ry="7"/>'))
add("frames", "frame-double", "Double Frame", "Frames", ["border", "outline", "nested"],
    frame('<rect x="2" y="3" width="20" height="18"/><rect x="5" y="6" width="14" height="12"/>'))
add("frames", "frame-corners", "Corner Brackets", "Frames", ["crop", "focus", "marker", "brackets"],
    frame('<polyline points="3,8 3,3 8,3"/><polyline points="16,3 21,3 21,8"/><polyline points="21,16 21,21 16,21"/><polyline points="8,21 3,21 3,16"/>'))
add("frames", "frame-portrait", "Portrait Frame", "Frames", ["border", "photo", "vertical"],
    frame('<rect x="5" y="2" width="14" height="20" rx="1"/>'))
add("frames", "frame-landscape", "Landscape Frame", "Frames", ["border", "photo", "horizontal"],
    frame('<rect x="2" y="5" width="20" height="14" rx="1"/>'))
add("frames", "frame-polaroid", "Polaroid Frame", "Frames", ["photo", "instant", "picture"],
    frame('<rect x="3" y="3" width="18" height="18"/><rect x="5.5" y="5.5" width="13" height="9.5"/>'))
add("frames", "frame-hexagon", "Hexagon Frame", "Frames", ["border", "outline", "polygon"],
    frame('<polygon points="6,3 18,3 23,12 18,21 6,21 1,12"/>'))
add("frames", "frame-octagon", "Octagon Frame", "Frames", ["border", "outline", "polygon"],
    frame('<polygon points="8,2 16,2 22,8 22,16 16,22 8,22 2,16 2,8"/>'))
add("frames", "frame-diamond", "Diamond Frame", "Frames", ["border", "outline", "rhombus"],
    frame('<polygon points="12,2 22,12 12,22 2,12"/>'))
add("frames", "frame-arch", "Arch Frame", "Frames", ["border", "window", "door"],
    frame('<path d="M4 21 V10 A8 8 0 0 1 20 10 V21 Z"/>'))
add("frames", "frame-ticket", "Ticket Frame", "Frames", ["coupon", "voucher", "pass"],
    frame('<path d="M3 6 H21 V10 A2 2 0 0 0 21 14 V18 H3 V14 A2 2 0 0 0 3 10 Z"/>'))
add("frames", "frame-tag", "Tag Frame", "Frames", ["label", "price", "sale"],
    frame('<path d="M3 9 V4 H8 L21 4 A0 0 0 0 1 21 4 L21 20 H8 L3 15 Z"/><path d="M3 9 V15 L8 20 M3 9 L8 4"/>'))
add("frames", "frame-banner", "Banner Frame", "Frames", ["ribbon", "header", "title"],
    frame('<path d="M3 6 H21 V16 H15 L12 19 L9 16 H3 Z"/>'))
add("frames", "frame-shield", "Shield Frame", "Frames", ["badge", "crest", "guard"],
    frame('<path d="M12 3 L20 6 V11 C20 16 16 19.5 12 21 C8 19.5 4 16 4 11 V6 Z"/>'))
add("frames", "frame-blob", "Blob Frame", "Frames", ["organic", "avatar", "abstract"],
    frame('<path d="M12 3 C17 3 21 6 21 11 C21 16 18 21 12 21 C7 21 3 17 3 12 C3 7 7 3 12 3 Z"/>'))
add("frames", "frame-dashed", "Dashed Frame", "Frames", ["border", "dotted", "placeholder"],
    svg(f'<rect x="3" y="4" width="18" height="16" rx="2" fill="none" stroke="{FRAME_STROKE}" stroke-width="2" stroke-dasharray="4 3"/>'))
add("frames", "frame-pill", "Pill Frame", "Frames", ["rounded", "stadium", "capsule"],
    frame('<rect x="2" y="7" width="20" height="10" rx="5"/>'))

# -- Dividers --------------------------------------------------------------
add("frames", "divider-line", "Line Divider", "Dividers", ["rule", "separator", "hr"],
    icon('<line x1="2" y1="12" x2="22" y2="12"/>'))
add("frames", "divider-dashed", "Dashed Divider", "Dividers", ["rule", "separator", "dash"],
    svg(f'<line x1="2" y1="12" x2="22" y2="12" fill="none" stroke="{ICON_STROKE}" stroke-width="2" stroke-linecap="round" stroke-dasharray="4 4"/>'))
add("frames", "divider-dotted", "Dotted Divider", "Dividers", ["rule", "separator", "dots"],
    svg(f'<line x1="2" y1="12" x2="22" y2="12" fill="none" stroke="{ICON_STROKE}" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="0.1 4"/>'))
add("frames", "divider-double", "Double Divider", "Dividers", ["rule", "separator", "parallel"],
    icon('<line x1="2" y1="10" x2="22" y2="10"/><line x1="2" y1="14" x2="22" y2="14"/>'))
add("frames", "divider-wavy", "Wavy Divider", "Dividers", ["rule", "separator", "wave"],
    icon('<path d="M2 12 C5 8 8 8 11 12 C14 16 17 16 22 12"/>'))
add("frames", "divider-zigzag", "Zigzag Divider", "Dividers", ["rule", "separator", "jagged"],
    icon('<polyline points="2,12 6,8 10,12 14,8 18,12 22,8"/>'))
add("frames", "divider-dots", "Dot Divider", "Dividers", ["separator", "ornament", "center"],
    icon('<line x1="2" y1="12" x2="8" y2="12"/><circle cx="12" cy="12" r="1"/><line x1="16" y1="12" x2="22" y2="12"/>'))
add("frames", "divider-diamond", "Diamond Divider", "Dividers", ["separator", "ornament", "center"],
    icon('<line x1="2" y1="12" x2="8" y2="12"/><path d="M12 9 L15 12 L12 15 L9 12 Z"/><line x1="16" y1="12" x2="22" y2="12"/>'))
add("frames", "divider-star", "Star Divider", "Dividers", ["separator", "ornament", "center"],
    icon('<line x1="2" y1="12" x2="7" y2="12"/><polygon points="12,8 13.2,11 16,11 13.8,13 14.6,16 12,14.2 9.4,16 10.2,13 8,11 10.8,11"/><line x1="17" y1="12" x2="22" y2="12"/>'))
add("frames", "divider-arrows", "Arrow Divider", "Dividers", ["separator", "ornament", "center"],
    icon('<line x1="2" y1="12" x2="9" y2="12"/><polyline points="11,9 14,12 11,15"/><line x1="15" y1="12" x2="22" y2="12"/>'))

# -- Badges / stickers (solid fills) ---------------------------------------
add("frames", "badge-burst-8", "Burst Badge", "Badges", ["sticker", "seal", "starburst", "sale"],
    shape(f'<polygon points="{star_points(8, ro=11, ri=8)}" fill="{AMBER}"/>'))
add("frames", "badge-burst-12", "Spiky Badge", "Badges", ["sticker", "seal", "starburst", "promo"],
    shape(f'<polygon points="{star_points(12, ro=11, ri=8.5)}" fill="{CORAL}"/>'))
add("frames", "badge-burst-16", "Sunburst Badge", "Badges", ["sticker", "seal", "starburst", "new"],
    shape(f'<polygon points="{star_points(16, ro=11, ri=9)}" fill="{INDIGO}"/>'))
add("frames", "badge-circle", "Circle Badge", "Badges", ["sticker", "seal", "round", "stamp"],
    shape(f'<circle cx="12" cy="12" r="10" fill="{INDIGO}"/><circle cx="12" cy="12" r="7.5" fill="none" stroke="{WHITE}" stroke-width="1"/>'))
add("frames", "badge-ribbon", "Ribbon Badge", "Badges", ["sticker", "award", "medal", "prize"],
    shape(f'<circle cx="12" cy="9" r="7" fill="{AMBER}"/><circle cx="12" cy="9" r="4.5" fill="{WHITE}"/><polygon points="8,14 6,22 12,19 18,22 16,14" fill="{CORAL}"/>'))
add("frames", "badge-shield", "Shield Badge", "Badges", ["sticker", "crest", "guard", "verified"],
    shape(f'<path d="M12 2 L21 5 V11 C21 16.5 17 20.5 12 22 C7 20.5 3 16.5 3 11 V5 Z" fill="{TEAL}"/>'))
add("frames", "badge-star", "Star Badge", "Badges", ["sticker", "rating", "favorite", "featured"],
    shape(f'<circle cx="12" cy="12" r="10" fill="{AMBER}"/><polygon points="12,5 14,10 19.5,10.3 15.2,14 16.6,19.3 12,16.2 7.4,19.3 8.8,14 4.5,10.3 10,10" fill="{WHITE}"/>'))
add("frames", "badge-hexagon", "Hexagon Badge", "Badges", ["sticker", "seal", "polygon", "stamp"],
    shape(f'<polygon points="6,3 18,3 23,12 18,21 6,21 1,12" fill="{VIOLET}"/><polygon points="8,6 16,6 19.5,12 16,18 8,18 4.5,12" fill="none" stroke="{WHITE}" stroke-width="1"/>'))
add("frames", "badge-price-tag", "Price Tag Badge", "Badges", ["sticker", "sale", "label", "offer"],
    shape(f'<path d="M3 9 V4 H8 L21 4 L21 20 H8 L3 15 Z" fill="{CORAL}"/><circle cx="7" cy="8" r="1.5" fill="{WHITE}"/>'))

# ==========================================================================
# FLAT ILLUSTRATIONS (multi-colour)
# ==========================================================================
# -- Scenes ----------------------------------------------------------------
add("illustrations", "mountain-sun", "Mountain & Sun", "Scenes", ["landscape", "nature", "scene", "outdoor", "hill"],
    shape(
        f'<rect x="1" y="2" width="22" height="20" rx="2" fill="{SKY}"/>'
        f'<circle cx="17" cy="8" r="3" fill="{SUN}"/>'
        f'<path d="M1 22 L8 11 L13 18 L16 14 L23 22 Z" fill="{GRASS}"/>'
        f'<path d="M8 11 L11.2 16 L4.8 16 Z" fill="{ROCK}"/>'
    ))
add("illustrations", "cloud-sun", "Sun & Cloud", "Weather", ["weather", "sky", "forecast", "day"],
    shape(
        f'<circle cx="9" cy="9" r="4" fill="{SUN}"/>'
        f'<path d="M11 19 A3.5 3.5 0 0 1 11 12 A4.5 4.5 0 0 1 20 11.5 A3 3 0 0 1 20 19 Z" fill="{WHITE}" stroke="{ROCK}" stroke-width="1"/>'
    ))
add("illustrations", "rocket-illo", "Rocket", "Objects", ["launch", "startup", "space", "boost", "fast"],
    shape(
        f'<path d="M12 2 C16 5 17 10 16 15 H8 C7 10 8 5 12 2 Z" fill="{PAPER}" stroke="{INK}" stroke-width="1"/>'
        f'<circle cx="12" cy="9" r="2" fill="{INDIGO}"/>'
        f'<path d="M8 13 L5 16 L8 15 Z" fill="{CORAL}"/>'
        f'<path d="M16 13 L19 16 L16 15 Z" fill="{CORAL}"/>'
        f'<path d="M10 15 H14 L13 20 L12 18 L11 20 Z" fill="{AMBER}"/>'
    ))
add("illustrations", "lightbulb-illo", "Idea", "Objects", ["bulb", "light", "innovation", "tip", "think"],
    shape(
        f'<path d="M12 2 A7 7 0 0 1 16 15 H8 A7 7 0 0 1 12 2 Z" fill="{SUN}"/>'
        f'<rect x="9" y="15" width="6" height="3" fill="{ROCK}"/>'
        f'<rect x="10" y="18" width="4" height="3" rx="1" fill="{INK}"/>'
    ))
add("illustrations", "target-illo", "Target", "Objects", ["goal", "aim", "bullseye", "focus", "objective"],
    shape(
        f'<circle cx="12" cy="12" r="10" fill="{CORAL}"/>'
        f'<circle cx="12" cy="12" r="6.5" fill="{PAPER}"/>'
        f'<circle cx="12" cy="12" r="3" fill="{CORAL}"/>'
    ))
add("illustrations", "chat-bubbles", "Chat Bubbles", "Objects", ["message", "conversation", "talk", "comment", "support"],
    shape(
        f'<path d="M2 4 H16 A2 2 0 0 1 18 6 V12 A2 2 0 0 1 16 14 H8 L4 17 V14 H2 Z" fill="{INDIGO}"/>'
        f'<path d="M22 10 V17 A2 2 0 0 1 20 19 H12 L9 21.5 V19 H10 A2 2 0 0 1 8 17 V16 H16 A2 2 0 0 0 18 14 H20 A2 2 0 0 1 22 10 Z" fill="{TEAL}"/>'
    ))
add("illustrations", "clipboard-check", "Checklist", "Objects", ["todo", "task", "list", "done", "clipboard"],
    shape(
        f'<rect x="4" y="3" width="16" height="19" rx="2" fill="{PAPER}" stroke="{ROCK}" stroke-width="1"/>'
        f'<rect x="9" y="2" width="6" height="3" rx="1" fill="{ROCK}"/>'
        f'<polyline points="7,10 9,12 12,8.5" fill="none" stroke="{GRASS}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<polyline points="7,16 9,18 12,14.5" fill="none" stroke="{GRASS}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<line x1="14" y1="11" x2="17" y2="11" stroke="{ROCK}" stroke-width="2" stroke-linecap="round"/>'
        f'<line x1="14" y1="17" x2="17" y2="17" stroke="{ROCK}" stroke-width="2" stroke-linecap="round"/>'
    ))
add("illustrations", "trophy-illo", "Trophy", "Objects", ["award", "win", "prize", "champion", "winner"],
    shape(
        f'<path d="M7 3 H17 V9 A5 5 0 0 1 7 9 Z" fill="{SUN}"/>'
        f'<path d="M7 5 H4 V7 A3 3 0 0 0 7 10" fill="none" stroke="{AMBER}" stroke-width="1.5"/>'
        f'<path d="M17 5 H20 V7 A3 3 0 0 1 17 10" fill="none" stroke="{AMBER}" stroke-width="1.5"/>'
        f'<rect x="10" y="13" width="4" height="4" fill="{AMBER}"/>'
        f'<rect x="7" y="17" width="10" height="3" rx="1" fill="{ROCK}"/>'
    ))
add("illustrations", "gift-illo", "Gift", "Objects", ["present", "reward", "box", "surprise", "birthday"],
    shape(
        f'<rect x="3" y="9" width="18" height="12" rx="1" fill="{INDIGO}"/>'
        f'<rect x="3" y="7" width="18" height="4" fill="{TEAL}"/>'
        f'<rect x="10.5" y="7" width="3" height="14" fill="{AMBER}"/>'
        f'<path d="M12 7 C9 7 7 3 9.5 3 C11.5 3 12 7 12 7 Z" fill="{AMBER}"/>'
        f'<path d="M12 7 C15 7 17 3 14.5 3 C12.5 3 12 7 12 7 Z" fill="{AMBER}"/>'
    ))
add("illustrations", "flag-illo", "Flag", "Objects", ["banner", "milestone", "marker", "country", "goal"],
    shape(
        f'<line x1="5" y1="2" x2="5" y2="22" stroke="{INK}" stroke-width="2" stroke-linecap="round"/>'
        f'<path d="M5 3 H20 L17 7.5 L20 12 H5 Z" fill="{CORAL}"/>'
    ))
add("illustrations", "location-map", "Map", "Scenes", ["location", "navigation", "place", "travel", "route"],
    shape(
        f'<path d="M2 5 L9 3 L15 5 L22 3 V19 L15 21 L9 19 L2 21 Z" fill="{GRASS}"/>'
        f'<path d="M9 3 V19 M15 5 V21" stroke="{WHITE}" stroke-width="1" fill="none"/>'
        f'<path d="M15 8 A3 3 0 0 1 18 11 C18 13.5 15 16 15 16 C15 16 12 13.5 12 11 A3 3 0 0 1 15 8 Z" fill="{CORAL}"/>'
    ))
add("illustrations", "coffee", "Coffee", "Objects", ["cup", "drink", "break", "cafe", "mug"],
    shape(
        f'<path d="M4 8 H18 V14 A5 5 0 0 1 8 14 Z" fill="{INK}"/>'
        f'<path d="M18 9 H20 A2 2 0 0 1 20 13 H18" fill="none" stroke="{INK}" stroke-width="1.5"/>'
        f'<rect x="6" y="19" width="14" height="2" rx="1" fill="{ROCK}"/>'
        f'<path d="M9 3 C9 4.5 8 4.5 8 6 M13 3 C13 4.5 12 4.5 12 6" fill="none" stroke="{ROCK}" stroke-width="1.2" stroke-linecap="round"/>'
    ))

# -- New scenes ------------------------------------------------------------
add("illustrations", "moon-stars", "Night Sky", "Scenes", ["night", "stars", "moon", "sky", "dark"],
    shape(
        f'<rect x="1" y="2" width="22" height="20" rx="2" fill="{INDIGO}"/>'
        f'<path d="M16 5 A6 6 0 1 0 16 17 A4.5 4.5 0 0 1 16 5 Z" fill="{SUN}"/>'
        f'<polygon points="6,7 6.6,8.4 8,9 6.6,9.6 6,11 5.4,9.6 4,9 5.4,8.4" fill="{WHITE}"/>'
        f'<polygon points="9,14 9.4,15 10.5,15.4 9.4,15.8 9,17 8.6,15.8 7.5,15.4 8.6,15" fill="{WHITE}"/>'
    ))
add("illustrations", "rainbow", "Rainbow", "Weather", ["weather", "colorful", "arc", "pride"],
    shape(
        f'<path d="M2 20 A10 10 0 0 1 22 20" fill="none" stroke="{CORAL}" stroke-width="2.4"/>'
        f'<path d="M4.4 20 A7.6 7.6 0 0 1 19.6 20" fill="none" stroke="{AMBER}" stroke-width="2.4"/>'
        f'<path d="M6.8 20 A5.2 5.2 0 0 1 17.2 20" fill="none" stroke="{GRASS}" stroke-width="2.4"/>'
        f'<path d="M9.2 20 A2.8 2.8 0 0 1 14.8 20" fill="none" stroke="{INDIGO}" stroke-width="2.4"/>'
    ))
add("illustrations", "cloud-rain-illo", "Rain Cloud", "Weather", ["weather", "rain", "storm", "shower"],
    shape(
        f'<path d="M7 14 A4.5 4.5 0 0 1 7 5 A5.5 5.5 0 0 1 17.5 4 A4 4 0 0 1 18 14 Z" fill="{SLATE}"/>'
        f'<line x1="8" y1="16" x2="7" y2="20" stroke="{INDIGO}" stroke-width="2" stroke-linecap="round"/>'
        f'<line x1="12" y1="16" x2="11" y2="21" stroke="{INDIGO}" stroke-width="2" stroke-linecap="round"/>'
        f'<line x1="16" y1="16" x2="15" y2="20" stroke="{INDIGO}" stroke-width="2" stroke-linecap="round"/>'
    ))
add("illustrations", "sun-illo", "Sunshine", "Weather", ["sun", "summer", "bright", "day"],
    shape(
        f'<circle cx="12" cy="12" r="6" fill="{SUN}"/>'
        f'<g stroke="{AMBER}" stroke-width="2" stroke-linecap="round">'
        f'<line x1="12" y1="1" x2="12" y2="4"/><line x1="12" y1="20" x2="12" y2="23"/>'
        f'<line x1="1" y1="12" x2="4" y2="12"/><line x1="20" y1="12" x2="23" y2="12"/>'
        f'<line x1="4" y1="4" x2="6" y2="6"/><line x1="18" y1="18" x2="20" y2="20"/>'
        f'<line x1="20" y1="4" x2="18" y2="6"/><line x1="6" y1="18" x2="4" y2="20"/></g>'
    ))

# -- Nature ----------------------------------------------------------------
add("illustrations", "tree-illo", "Tree", "Nature", ["plant", "forest", "nature", "park"],
    shape(
        f'<rect x="10.5" y="14" width="3" height="7" fill="{WOOD}"/>'
        f'<circle cx="12" cy="9" r="6" fill="{LEAF}"/>'
        f'<circle cx="8" cy="11" r="3.5" fill="{GRASS}"/>'
        f'<circle cx="16" cy="11" r="3.5" fill="{GRASS}"/>'
    ))
add("illustrations", "plant-pot", "Potted Plant", "Nature", ["plant", "pot", "indoor", "leaf", "green"],
    shape(
        f'<path d="M7 14 H17 L15.5 21 H8.5 Z" fill="{CORAL}"/>'
        f'<path d="M12 14 C12 9 9 6 6 6 C6 11 8 14 12 14 Z" fill="{LEAF}"/>'
        f'<path d="M12 14 C12 8 15 4 19 4 C19 10 16 14 12 14 Z" fill="{GRASS}"/>'
    ))
add("illustrations", "flower", "Flower", "Nature", ["bloom", "petal", "spring", "garden"],
    shape(
        f'<circle cx="12" cy="9" r="2.5" fill="{SUN}"/>'
        f'<g fill="{CORAL}"><circle cx="12" cy="4" r="2.6"/><circle cx="12" cy="14" r="2.6"/>'
        f'<circle cx="7" cy="9" r="2.6"/><circle cx="17" cy="9" r="2.6"/></g>'
        f'<circle cx="12" cy="9" r="2.5" fill="{SUN}"/>'
        f'<line x1="12" y1="14" x2="12" y2="22" stroke="{GRASS}" stroke-width="2"/>'
    ))
add("illustrations", "leaf-illo", "Leaf", "Nature", ["plant", "eco", "nature", "green"],
    shape(
        f'<path d="M4 20 C4 9 12 4 20 4 C20 15 13 20 4 20 Z" fill="{LEAF}"/>'
        f'<path d="M4 20 C9 15 14 11 19 7" fill="none" stroke="{GRASS}" stroke-width="1.4"/>'
    ))
add("illustrations", "water-drop", "Water Drop", "Nature", ["water", "drop", "liquid", "eco"],
    shape(
        f'<path d="M12 2 C12 2 5 11 5 15 A7 7 0 0 0 19 15 C19 11 12 2 12 2 Z" fill="{INDIGO}"/>'
        f'<path d="M9 15 A3 3 0 0 0 12 18" fill="none" stroke="{WHITE}" stroke-width="1.4" stroke-linecap="round"/>'
    ))
add("illustrations", "fire-illo", "Campfire", "Nature", ["fire", "flame", "hot", "warm"],
    shape(
        f'<path d="M12 3 C14 7 18 9 17 14 A5 5 0 0 1 7 14 C7 11 9 11 9.5 8 C11 10 12 9 12 7 Z" fill="{CORAL}"/>'
        f'<path d="M12 8 C13 10 15 11 14.5 14 A2.5 2.5 0 0 1 9.5 14 C9.5 12 11 12 12 8 Z" fill="{SUN}"/>'
    ))

# -- Objects ---------------------------------------------------------------
add("illustrations", "book-stack", "Books", "Objects", ["read", "library", "study", "learn"],
    shape(
        f'<rect x="3" y="16" width="18" height="4" rx="1" fill="{CORAL}"/>'
        f'<rect x="4" y="11" width="16" height="4" rx="1" fill="{TEAL}"/>'
        f'<rect x="5" y="6" width="14" height="4" rx="1" fill="{AMBER}"/>'
    ))
add("illustrations", "laptop-illo", "Laptop", "Objects", ["computer", "device", "work", "tech"],
    shape(
        f'<rect x="5" y="5" width="14" height="9" rx="1" fill="{SLATE}"/>'
        f'<rect x="6.5" y="6.5" width="11" height="6" fill="{SKY}"/>'
        f'<path d="M3 18 L21 18 L19 15 H5 Z" fill="{ROCK}"/>'
    ))
add("illustrations", "phone-illo", "Phone", "Objects", ["mobile", "device", "smartphone", "app"],
    shape(
        f'<rect x="7" y="2" width="10" height="20" rx="2" fill="{INK}"/>'
        f'<rect x="8" y="5" width="8" height="13" fill="{SKY}"/>'
        f'<circle cx="12" cy="20" r="0.8" fill="{WHITE}"/>'
    ))
add("illustrations", "envelope-illo", "Envelope", "Objects", ["mail", "email", "message", "letter"],
    shape(
        f'<rect x="2" y="5" width="20" height="14" rx="1" fill="{INDIGO}"/>'
        f'<path d="M2 6 L12 13 L22 6" fill="none" stroke="{WHITE}" stroke-width="1.4"/>'
    ))
add("illustrations", "calendar-illo", "Calendar", "Objects", ["date", "schedule", "event", "plan"],
    shape(
        f'<rect x="3" y="4" width="18" height="17" rx="2" fill="{PAPER}" stroke="{ROCK}" stroke-width="1"/>'
        f'<rect x="3" y="4" width="18" height="5" rx="2" fill="{CORAL}"/>'
        f'<line x1="8" y1="2" x2="8" y2="6" stroke="{ROCK}" stroke-width="2" stroke-linecap="round"/>'
        f'<line x1="16" y1="2" x2="16" y2="6" stroke="{ROCK}" stroke-width="2" stroke-linecap="round"/>'
        f'<rect x="6" y="12" width="3" height="3" fill="{TEAL}"/>'
        f'<rect x="11" y="12" width="3" height="3" fill="{TEAL}"/>'
    ))
add("illustrations", "key-illo", "Key", "Objects", ["unlock", "access", "password", "secure"],
    shape(
        f'<circle cx="8" cy="8" r="5" fill="none" stroke="{AMBER}" stroke-width="2.6"/>'
        f'<path d="M11 11 L20 20 L20 17 L17 17 L17 14" fill="none" stroke="{AMBER}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>'
    ))
add("illustrations", "balloon", "Balloon", "Objects", ["party", "celebrate", "birthday", "float"],
    shape(
        f'<ellipse cx="12" cy="8" rx="6" ry="7" fill="{CORAL}"/>'
        f'<polygon points="11,15 13,15 12,17" fill="{CORAL}"/>'
        f'<path d="M12 17 C12 19 14 19 13 22" fill="none" stroke="{ROCK}" stroke-width="1"/>'
    ))
add("illustrations", "cake", "Cake", "Objects", ["birthday", "party", "dessert", "celebrate"],
    shape(
        f'<rect x="4" y="12" width="16" height="8" rx="1" fill="{BLUSH}"/>'
        f'<path d="M4 14 C7 17 9 14 12 14 C15 14 17 17 20 14 V13 H4 Z" fill="{WHITE}"/>'
        f'<line x1="12" y1="6" x2="12" y2="12" stroke="{AMBER}" stroke-width="2"/>'
        f'<circle cx="12" cy="5" r="1.4" fill="{SUN}"/>'
    ))
add("illustrations", "crown-illo", "Crown", "Objects", ["king", "royal", "premium", "vip"],
    shape(
        f'<path d="M3 8 L7 13 L12 5 L17 13 L21 8 L19 20 H5 Z" fill="{SUN}"/>'
        f'<circle cx="3" cy="8" r="1.6" fill="{AMBER}"/>'
        f'<circle cx="21" cy="8" r="1.6" fill="{AMBER}"/>'
        f'<circle cx="12" cy="5" r="1.6" fill="{AMBER}"/>'
        f'<rect x="5" y="17" width="14" height="3" fill="{AMBER}"/>'
    ))
add("illustrations", "medal", "Medal", "Objects", ["award", "win", "prize", "first"],
    shape(
        f'<polygon points="8,2 10,2 13,9 9,9" fill="{INDIGO}"/>'
        f'<polygon points="14,2 16,2 15,9 11,9" fill="{CORAL}"/>'
        f'<circle cx="12" cy="15" r="6" fill="{SUN}"/>'
        f'<circle cx="12" cy="15" r="3.5" fill="{AMBER}"/>'
    ))
add("illustrations", "paper-plane", "Paper Plane", "Objects", ["send", "message", "fly", "launch"],
    shape(
        f'<path d="M2 12 L22 3 L15 21 L11 14 Z" fill="{SKY}"/>'
        f'<path d="M2 12 L11 14 L22 3 Z" fill="{INDIGO}"/>'
    ))
add("illustrations", "megaphone", "Megaphone", "Objects", ["announce", "marketing", "promo", "shout"],
    shape(
        f'<path d="M3 10 L14 6 V18 L3 14 Z" fill="{CORAL}"/>'
        f'<path d="M14 6 L20 4 V20 L14 18 Z" fill="{AMBER}"/>'
        f'<rect x="4" y="14" width="3" height="6" fill="{ROCK}"/>'
    ))
add("illustrations", "compass-illo", "Compass", "Objects", ["navigate", "explore", "direction", "travel"],
    shape(
        f'<circle cx="12" cy="12" r="10" fill="{SKY}" stroke="{SLATE}" stroke-width="1"/>'
        f'<polygon points="12,6 14,12 12,11" fill="{CORAL}"/>'
        f'<polygon points="12,18 10,12 12,13" fill="{WHITE}"/>'
        f'<circle cx="12" cy="12" r="1.4" fill="{INK}"/>'
    ))
add("illustrations", "shopping-bag-illo", "Shopping Bag", "Objects", ["shop", "buy", "retail", "sale"],
    shape(
        f'<path d="M5 8 H19 L20 21 H4 Z" fill="{TEAL}"/>'
        f'<path d="M8 8 V6 A4 4 0 0 1 16 6 V8" fill="none" stroke="{INK}" stroke-width="1.6"/>'
    ))

# -- Chart primitives ------------------------------------------------------
add("illustrations", "chart-bars-illo", "Bar Chart", "Charts", ["graph", "stats", "analytics", "bars", "data"],
    shape(
        f'<line x1="3" y1="21" x2="21" y2="21" stroke="{ROCK}" stroke-width="1.4"/>'
        f'<rect x="4" y="12" width="3.5" height="9" fill="{INDIGO}"/>'
        f'<rect x="9" y="7" width="3.5" height="14" fill="{TEAL}"/>'
        f'<rect x="14" y="14" width="3.5" height="7" fill="{CORAL}"/>'
        f'<rect x="19" y="9" width="2.5" height="12" fill="{AMBER}"/>'
    ))
add("illustrations", "chart-donut-illo", "Donut Chart", "Charts", ["graph", "stats", "pie", "ring", "data"],
    shape(
        f'<circle cx="12" cy="12" r="9" fill="{SKY}"/>'
        f'<path d="M12 3 A9 9 0 0 1 20.5 15 L12 12 Z" fill="{INDIGO}"/>'
        f'<path d="M20.5 15 A9 9 0 0 1 6 19.8 L12 12 Z" fill="{TEAL}"/>'
        f'<circle cx="12" cy="12" r="4.5" fill="{WHITE}"/>'
    ))
add("illustrations", "chart-line-illo", "Line Chart", "Charts", ["graph", "stats", "trend", "growth", "data"],
    shape(
        f'<rect x="2" y="3" width="20" height="18" rx="1" fill="{PAPER}"/>'
        f'<polyline points="4,17 9,11 13,14 20,5" fill="none" stroke="{INDIGO}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<circle cx="9" cy="11" r="1.4" fill="{CORAL}"/>'
        f'<circle cx="20" cy="5" r="1.4" fill="{CORAL}"/>'
    ))
add("illustrations", "chart-area-illo", "Area Chart", "Charts", ["graph", "stats", "trend", "data"],
    shape(
        f'<rect x="2" y="3" width="20" height="18" rx="1" fill="{PAPER}"/>'
        f'<path d="M3 20 V14 L8 9 L13 13 L21 5 V20 Z" fill="{TEAL}"/>'
        f'<polyline points="3,14 8,9 13,13 21,5" fill="none" stroke="{INDIGO}" stroke-width="1.6"/>'
    ))
add("illustrations", "chart-pie-illo", "Pie Chart", "Charts", ["graph", "stats", "slice", "data"],
    shape(
        f'<circle cx="12" cy="12" r="9" fill="{SKY}"/>'
        f'<path d="M12 12 L12 3 A9 9 0 0 1 21 12 Z" fill="{CORAL}"/>'
        f'<path d="M12 12 L21 12 A9 9 0 0 1 12 21 Z" fill="{AMBER}"/>'
    ))
add("illustrations", "gauge-illo", "Gauge", "Charts", ["meter", "speed", "kpi", "dial", "performance"],
    shape(
        f'<path d="M3 18 A9 9 0 0 1 21 18" fill="none" stroke="{SKY}" stroke-width="3"/>'
        f'<path d="M3 18 A9 9 0 0 1 8 9.8" fill="none" stroke="{GRASS}" stroke-width="3"/>'
        f'<line x1="12" y1="18" x2="16" y2="11" stroke="{INK}" stroke-width="2" stroke-linecap="round"/>'
        f'<circle cx="12" cy="18" r="1.6" fill="{INK}"/>'
    ))
add("illustrations", "chart-stacked-illo", "Stacked Bars", "Charts", ["graph", "stats", "stack", "data"],
    shape(
        f'<line x1="3" y1="21" x2="21" y2="21" stroke="{ROCK}" stroke-width="1.4"/>'
        f'<rect x="5" y="14" width="4" height="7" fill="{INDIGO}"/>'
        f'<rect x="5" y="9" width="4" height="5" fill="{TEAL}"/>'
        f'<rect x="13" y="12" width="4" height="9" fill="{INDIGO}"/>'
        f'<rect x="13" y="6" width="4" height="6" fill="{TEAL}"/>'
    ))
add("illustrations", "progress-ring", "Progress Ring", "Charts", ["loading", "percent", "circular", "kpi"],
    shape(
        f'<circle cx="12" cy="12" r="8" fill="none" stroke="{SKY}" stroke-width="3"/>'
        f'<path d="M12 4 A8 8 0 0 1 18 18" fill="none" stroke="{INDIGO}" stroke-width="3" stroke-linecap="round"/>'
    ))


# ==========================================================================
# Emit SVG files + the Rust catalog table.
# ==========================================================================
ID_RE = re.compile(r"^[a-z0-9-]+$")

VARIANT = {
    "shapes": "Shapes",
    "lines": "Lines",
    "icons": "Icons",
    "frames": "Frames",
    "illustrations": "Illustrations",
}


def main():
    rows = []
    # ids must be unique across the WHOLE catalogue (not just per category):
    # `AssetDef::id` is the lookup key for `assets::get` / insert, so a clash
    # between e.g. an icon and an illustration would shadow one of them.
    all_ids = set()
    total = 0
    for cat, items in ASSETS.items():
        cat_dir = os.path.join(DATA, cat)
        os.makedirs(cat_dir, exist_ok=True)
        expected = {f"{_id}.svg" for _id, *_ in items}
        for _id, name, group, tags, svg_text in items:
            assert ID_RE.match(_id), f"bad id {_id!r}"
            assert group, f"asset {_id!r} has no group"
            assert _id not in all_ids, f"duplicate id {_id!r} (must be globally unique)"
            all_ids.add(_id)
            with open(os.path.join(cat_dir, f"{_id}.svg"), "w", encoding="utf-8") as fh:
                fh.write(svg_text)
            tag_lits = ", ".join(f'"{t}"' for t in tags)
            rows.append(
                f'    AssetDef {{\n'
                f'        id: "{_id}",\n'
                f'        name: "{name}",\n'
                f'        category: AssetCategory::{VARIANT[cat]},\n'
                f'        group: "{group}",\n'
                f'        tags: &[{tag_lits}],\n'
                f'        svg: include_str!("data/{cat}/{_id}.svg"),\n'
                f'    }},'
            )
            total += 1
        # Prune stale SVGs left behind by renamed/removed assets so the
        # on-disk data/ tree always matches the generated catalogue.
        for existing in os.listdir(cat_dir):
            if existing.endswith(".svg") and existing not in expected:
                os.remove(os.path.join(cat_dir, existing))

    header = (
        "// @generated by generate_assets.py — DO NOT EDIT BY HAND.\n"
        "//\n"
        "// Run `python3 generate_assets.py` from this directory to\n"
        "// regenerate after changing the asset list in that script.\n"
        "//\n"
        f"// {total} bundled assets across {len(ASSETS)} categories.\n\n"
        "use super::{AssetCategory, AssetDef};\n\n"
        "pub(super) const ASSET_DEFS: &[AssetDef] = &[\n"
    )
    with open(os.path.join(HERE, "catalog.rs"), "w", encoding="utf-8") as fh:
        fh.write(header)
        fh.write("\n".join(rows))
        fh.write("\n];\n")

    print(f"wrote {total} assets across {len(ASSETS)} categories")
    for cat, items in ASSETS.items():
        print(f"  {cat}: {len(items)}")


if __name__ == "__main__":
    main()
