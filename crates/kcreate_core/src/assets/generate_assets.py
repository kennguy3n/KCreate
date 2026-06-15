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
#   * Shapes / illustrations: solid fills.
# All colours are plain neutral defaults; inserted nodes are fully
# recolorable in the editor.

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


def frame(body):
    return svg(
        f'<g fill="none" stroke="{FRAME_STROKE}" stroke-width="2" '
        f'stroke-linecap="round" stroke-linejoin="round">{body}</g>'
    )


def shape(body):
    return svg(body)


# Each entry: (id, name, [tags], svg_text)
ASSETS = {"shapes": [], "lines": [], "icons": [], "frames": [], "illustrations": []}


def add(cat, _id, name, tags, svg_text):
    ASSETS[cat].append((_id, name, tags, svg_text))


# --------------------------------------------------------------------------
# Basic shapes
# --------------------------------------------------------------------------
add("shapes", "rectangle", "Rectangle", ["square", "box", "rect", "block"],
    shape(f'<rect x="2" y="5" width="20" height="14" fill="{SHAPE_FILL}"/>'))
add("shapes", "rounded-rectangle", "Rounded Rectangle", ["rounded", "box", "card", "rect"],
    shape(f'<rect x="2" y="5" width="20" height="14" rx="3" fill="{SHAPE_FILL}"/>'))
add("shapes", "square", "Square", ["box", "rect", "tile"],
    shape(f'<rect x="4" y="4" width="16" height="16" fill="{SHAPE_FILL}"/>'))
add("shapes", "circle", "Circle", ["round", "dot", "ellipse", "ball"],
    shape(f'<circle cx="12" cy="12" r="10" fill="{SHAPE_FILL}"/>'))
add("shapes", "ellipse", "Ellipse", ["oval", "round", "circle"],
    shape(f'<ellipse cx="12" cy="12" rx="11" ry="7" fill="{SHAPE_FILL}"/>'))
add("shapes", "triangle", "Triangle", ["delta", "pyramid", "polygon"],
    shape(f'<polygon points="12,3 22,21 2,21" fill="{SHAPE_FILL}"/>'))
add("shapes", "right-triangle", "Right Triangle", ["corner", "ramp", "polygon"],
    shape(f'<polygon points="3,3 3,21 21,21" fill="{SHAPE_FILL}"/>'))
add("shapes", "diamond", "Diamond", ["rhombus", "gem", "kite"],
    shape(f'<polygon points="12,2 22,12 12,22 2,12" fill="{SHAPE_FILL}"/>'))
add("shapes", "pentagon", "Pentagon", ["polygon", "five"],
    shape(f'<polygon points="12,2 22,9.5 18,21.5 6,21.5 2,9.5" fill="{SHAPE_FILL}"/>'))
add("shapes", "hexagon", "Hexagon", ["polygon", "six", "honeycomb"],
    shape(f'<polygon points="6,3 18,3 23,12 18,21 6,21 1,12" fill="{SHAPE_FILL}"/>'))
add("shapes", "octagon", "Octagon", ["polygon", "stop", "eight"],
    shape(f'<polygon points="8,2 16,2 22,8 22,16 16,22 8,22 2,16 2,8" fill="{SHAPE_FILL}"/>'))
add("shapes", "star", "Star", ["rating", "favorite", "five", "polygon"],
    shape(f'<polygon points="12,2 15,9 22.5,9.3 16.5,14 18.5,21.5 12,17.2 5.5,21.5 7.5,14 1.5,9.3 9,9" fill="{SHAPE_FILL}"/>'))
add("shapes", "star-six", "Six-Point Star", ["sparkle", "polygon", "burst"],
    shape(f'<path d="M12 1 L15 7 L22 7 L17 12 L22 17 L15 17 L12 23 L9 17 L2 17 L7 12 L2 7 L9 7 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "speech-bubble", "Speech Bubble", ["chat", "message", "comment", "talk"],
    shape(f'<path d="M3 4 H21 A2 2 0 0 1 23 6 V15 A2 2 0 0 1 21 17 H10 L5 22 V17 H3 A2 2 0 0 1 1 15 V6 A2 2 0 0 1 3 4 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "speech-bubble-round", "Rounded Speech Bubble", ["chat", "message", "comment", "talk"],
    shape(f'<path d="M12 3 C5.4 3 1 7 1 11.5 C1 14 2.5 16.2 5 17.6 C4.7 19 3.8 20.4 2.5 21.5 C5 21.4 7.4 20.5 9.2 19.3 C10.1 19.5 11 19.6 12 19.6 C18.6 19.6 23 15.6 23 11.5 C23 7 18.6 3 12 3 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "heart", "Heart", ["love", "like", "favorite"],
    shape(f'<path d="M12 21 C12 21 2 14.5 2 8.2 C2 5 4.4 3 7.2 3 C9 3 10.6 4 12 5.8 C13.4 4 15 3 16.8 3 C19.6 3 22 5 22 8.2 C22 14.5 12 21 12 21 Z" fill="{CORAL}"/>'))
add("shapes", "plus-cross", "Cross", ["plus", "add", "medical"],
    shape(f'<path d="M9 2 H15 V9 H22 V15 H15 V22 H9 V15 H2 V9 H9 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "parallelogram", "Parallelogram", ["polygon", "slant", "skew"],
    shape(f'<polygon points="6,5 22,5 18,19 2,19" fill="{SHAPE_FILL}"/>'))
add("shapes", "trapezoid", "Trapezoid", ["polygon", "trapezium"],
    shape(f'<polygon points="6,5 18,5 22,19 2,19" fill="{SHAPE_FILL}"/>'))
add("shapes", "semicircle", "Semicircle", ["half", "round", "dome"],
    shape(f'<path d="M2 18 A10 10 0 0 1 22 18 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "badge", "Badge Seal", ["seal", "award", "burst", "starburst", "certified"],
    shape(f'<path d="M12 1 L14.3 4.1 L18 2.8 L18.2 6.7 L22 7.5 L19.9 10.8 L22.4 13.8 L18.8 15.4 L19 19.3 L15.2 18.4 L13.3 21.8 L12 18.2 L10.7 21.8 L8.8 18.4 L5 19.3 L5.2 15.4 L1.6 13.8 L4.1 10.8 L2 7.5 L5.8 6.7 L6 2.8 L9.7 4.1 Z" fill="{AMBER}"/>'))
add("shapes", "shield", "Shield", ["security", "protect", "guard", "badge"],
    shape(f'<path d="M12 2 L21 5 V11 C21 16.5 17 20.5 12 22 C7 20.5 3 16.5 3 11 V5 Z" fill="{SHAPE_FILL}"/>'))
add("shapes", "arrow-block", "Block Arrow", ["arrow", "right", "direction", "next"],
    shape(f'<path d="M2 9 H13 V5 L22 12 L13 19 V15 H2 Z" fill="{SHAPE_FILL}"/>'))

# --------------------------------------------------------------------------
# Lines / arrows / connectors
# --------------------------------------------------------------------------
add("lines", "line-horizontal", "Horizontal Line", ["rule", "divider", "straight"],
    icon('<line x1="2" y1="12" x2="22" y2="12"/>'))
add("lines", "line-vertical", "Vertical Line", ["rule", "divider", "straight"],
    icon('<line x1="12" y1="2" x2="12" y2="22"/>'))
add("lines", "line-diagonal", "Diagonal Line", ["slash", "straight"],
    icon('<line x1="3" y1="21" x2="21" y2="3"/>'))
add("lines", "line-dashed", "Dashed Line", ["dash", "divider", "rule"],
    svg(f'<line x1="2" y1="12" x2="22" y2="12" fill="none" stroke="{ICON_STROKE}" stroke-width="2" stroke-linecap="round" stroke-dasharray="4 4"/>'))
add("lines", "line-dotted", "Dotted Line", ["dots", "divider", "rule"],
    svg(f'<line x1="2" y1="12" x2="22" y2="12" fill="none" stroke="{ICON_STROKE}" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="0.1 4"/>'))
add("lines", "arrow-right", "Arrow Right", ["next", "forward", "direction"],
    icon('<line x1="3" y1="12" x2="20" y2="12"/><polyline points="14,6 20,12 14,18"/>'))
add("lines", "arrow-left", "Arrow Left", ["back", "previous", "direction"],
    icon('<line x1="21" y1="12" x2="4" y2="12"/><polyline points="10,6 4,12 10,18"/>'))
add("lines", "arrow-up", "Arrow Up", ["top", "north", "direction"],
    icon('<line x1="12" y1="21" x2="12" y2="4"/><polyline points="6,10 12,4 18,10"/>'))
add("lines", "arrow-down", "Arrow Down", ["bottom", "south", "direction"],
    icon('<line x1="12" y1="3" x2="12" y2="20"/><polyline points="6,14 12,20 18,14"/>'))
add("lines", "arrow-double", "Double Arrow", ["both", "swap", "exchange"],
    icon('<line x1="3" y1="12" x2="21" y2="12"/><polyline points="7,8 3,12 7,16"/><polyline points="17,8 21,12 17,16"/>'))
add("lines", "arrow-curved", "Curved Arrow", ["bend", "redo", "turn"],
    icon('<path d="M4 18 C4 10 9 6 19 6"/><polyline points="14,3 20,6 16,11"/>'))
add("lines", "arrow-elbow", "Elbow Arrow", ["connector", "corner", "turn", "right-angle"],
    icon('<polyline points="4,5 4,16 18,16"/><polyline points="13,11 19,16 13,21"/>'))
add("lines", "connector-elbow", "Elbow Connector", ["connector", "flow", "link", "step"],
    icon('<polyline points="3,6 12,6 12,18 21,18"/>'))
add("lines", "connector-curved", "Curved Connector", ["connector", "flow", "link", "bezier"],
    icon('<path d="M3 6 C12 6 12 18 21 18"/>'))
add("lines", "zigzag", "Zigzag", ["wave", "line", "lightning"],
    icon('<polyline points="2,16 7,8 12,16 17,8 22,16"/>'))
add("lines", "wave-line", "Wave Line", ["wave", "curve", "squiggle"],
    icon('<path d="M2 12 C5 6 8 6 11 12 C14 18 17 18 20 12"/>'))

# --------------------------------------------------------------------------
# Icon set (stroke style)
# --------------------------------------------------------------------------
add("icons", "home", "Home", ["house", "main", "start", "dashboard"],
    icon('<path d="M3 11 L12 3 L21 11"/><path d="M5 9.5 V20 H19 V9.5"/><rect x="10" y="14" width="4" height="6"/>'))
add("icons", "search", "Search", ["find", "magnify", "look", "zoom"],
    icon('<circle cx="11" cy="11" r="7"/><line x1="16" y1="16" x2="21" y2="21"/>'))
add("icons", "settings", "Settings", ["gear", "cog", "preferences", "options"],
    icon('<circle cx="12" cy="12" r="3"/><path d="M12 2 L12 5 M12 19 L12 22 M2 12 L5 12 M19 12 L22 12 M4.9 4.9 L7 7 M17 17 L19.1 19.1 M19.1 4.9 L17 7 M7 17 L4.9 19.1"/>'))
add("icons", "sliders", "Sliders", ["adjust", "controls", "filter", "settings"],
    icon('<line x1="4" y1="6" x2="20" y2="6"/><line x1="4" y1="12" x2="20" y2="12"/><line x1="4" y1="18" x2="20" y2="18"/><circle cx="9" cy="6" r="2"/><circle cx="15" cy="12" r="2"/><circle cx="8" cy="18" r="2"/>'))
add("icons", "user", "User", ["person", "profile", "account", "avatar"],
    icon('<circle cx="12" cy="8" r="4"/><path d="M4 21 C4 16 8 14 12 14 C16 14 20 16 20 21"/>'))
add("icons", "users", "Users", ["people", "group", "team", "contacts"],
    icon('<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20 C2.5 15.5 6 13.5 9 13.5 C12 13.5 15.5 15.5 15.5 20"/><path d="M16 5 A3.5 3.5 0 0 1 16 12"/><path d="M17 13.7 C19.4 14.3 21.5 16.2 21.5 20"/>'))
add("icons", "heart-outline", "Heart Outline", ["love", "like", "favorite"],
    icon('<path d="M12 20 C12 20 3 14 3 8.5 C3 6 5 4 7.5 4 C9.2 4 10.8 5 12 6.8 C13.2 5 14.8 4 16.5 4 C19 4 21 6 21 8.5 C21 14 12 20 12 20 Z"/>'))
add("icons", "star-outline", "Star Outline", ["rating", "favorite", "bookmark"],
    icon('<polygon points="12,3 14.6,8.6 21,9.2 16,13.6 17.6,20 12,16.5 6.4,20 8,13.6 3,9.2 9.4,8.6"/>'))
add("icons", "share", "Share", ["send", "social", "network", "export"],
    icon('<circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><line x1="8.2" y1="10.8" x2="15.8" y2="7.2"/><line x1="8.2" y1="13.2" x2="15.8" y2="16.8"/>'))
add("icons", "chevron-up", "Chevron Up", ["arrow", "collapse", "up"],
    icon('<polyline points="5,15 12,8 19,15"/>'))
add("icons", "chevron-down", "Chevron Down", ["arrow", "expand", "down", "more"],
    icon('<polyline points="5,9 12,16 19,9"/>'))
add("icons", "chevron-left", "Chevron Left", ["arrow", "back", "previous"],
    icon('<polyline points="15,5 8,12 15,19"/>'))
add("icons", "chevron-right", "Chevron Right", ["arrow", "next", "forward"],
    icon('<polyline points="9,5 16,12 9,19"/>'))
add("icons", "check", "Check", ["tick", "done", "ok", "success", "checkmark"],
    icon('<polyline points="4,12 10,18 20,6"/>'))
add("icons", "check-circle", "Check Circle", ["tick", "done", "ok", "success", "verified"],
    icon('<circle cx="12" cy="12" r="9"/><polyline points="8,12 11,15 16,9"/>'))
add("icons", "close", "Close", ["x", "cancel", "remove", "delete", "exit"],
    icon('<line x1="5" y1="5" x2="19" y2="19"/><line x1="19" y1="5" x2="5" y2="19"/>'))
add("icons", "x-circle", "Close Circle", ["x", "cancel", "error", "remove"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="9" y1="9" x2="15" y2="15"/><line x1="15" y1="9" x2="9" y2="15"/>'))
add("icons", "plus", "Plus", ["add", "new", "create", "more"],
    icon('<line x1="12" y1="4" x2="12" y2="20"/><line x1="4" y1="12" x2="20" y2="12"/>'))
add("icons", "minus", "Minus", ["remove", "subtract", "less"],
    icon('<line x1="4" y1="12" x2="20" y2="12"/>'))
add("icons", "bell", "Bell", ["notification", "alert", "alarm", "reminder"],
    icon('<path d="M6 16 V11 A6 6 0 0 1 18 11 V16 L20 18 H4 Z"/><path d="M10 18 A2 2 0 0 0 14 18"/>'))
add("icons", "camera", "Camera", ["photo", "picture", "capture", "image"],
    icon('<path d="M3 7 H7 L9 5 H15 L17 7 H21 V19 H3 Z"/><circle cx="12" cy="13" r="3.5"/>'))
add("icons", "image", "Image", ["photo", "picture", "gallery", "media"],
    icon('<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8.5" cy="9" r="1.8"/><polyline points="4,18 10,12 14,16 17,13 20,16"/>'))
add("icons", "calendar", "Calendar", ["date", "schedule", "event", "month"],
    icon('<rect x="3" y="5" width="18" height="16" rx="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="8" y1="3" x2="8" y2="7"/><line x1="16" y1="3" x2="16" y2="7"/>'))
add("icons", "clock", "Clock", ["time", "watch", "schedule", "timer"],
    icon('<circle cx="12" cy="12" r="9"/><polyline points="12,7 12,12 16,14"/>'))
add("icons", "chart-bar", "Bar Chart", ["graph", "stats", "analytics", "data", "bars"],
    icon('<line x1="3" y1="21" x2="21" y2="21"/><rect x="5" y="11" width="3.5" height="9"/><rect x="10.2" y="6" width="3.5" height="14"/><rect x="15.5" y="14" width="3.5" height="6"/>'))
add("icons", "chart-line", "Line Chart", ["graph", "stats", "analytics", "data", "trend"],
    icon('<polyline points="3,17 9,11 13,14 21,5"/><polyline points="16,5 21,5 21,10"/>'))
add("icons", "chart-pie", "Pie Chart", ["graph", "stats", "analytics", "data", "donut"],
    icon('<path d="M12 3 A9 9 0 1 1 3 12 H12 Z"/><path d="M12 3 V12 H21 A9 9 0 0 0 12 3 Z"/>'))
add("icons", "cart", "Shopping Cart", ["shop", "buy", "store", "ecommerce", "basket"],
    icon('<circle cx="9" cy="20" r="1.5"/><circle cx="18" cy="20" r="1.5"/><path d="M2 3 H5 L7 15 H19 L21 7 H6"/>'))
add("icons", "lock", "Lock", ["secure", "private", "password", "locked"],
    icon('<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M8 10 V7 A4 4 0 0 1 16 7 V10"/>'))
add("icons", "unlock", "Unlock", ["secure", "open", "access", "unlocked"],
    icon('<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M8 10 V7 A4 4 0 0 1 15.5 5"/>'))
add("icons", "mail", "Mail", ["email", "envelope", "message", "inbox"],
    icon('<rect x="3" y="5" width="18" height="14" rx="2"/><polyline points="3,7 12,13 21,7"/>'))
add("icons", "phone", "Phone", ["call", "contact", "telephone", "mobile"],
    icon('<path d="M5 3 H9 L11 8 L8.5 10.5 C9.5 12.5 11.5 14.5 13.5 15.5 L16 13 L21 15 V19 A2 2 0 0 1 19 21 C10 21 3 14 3 5 A2 2 0 0 1 5 3 Z"/>'))
add("icons", "map-pin", "Map Pin", ["location", "place", "marker", "gps", "pin"],
    icon('<path d="M12 22 C12 22 5 14.5 5 9 A7 7 0 0 1 19 9 C19 14.5 12 22 12 22 Z"/><circle cx="12" cy="9" r="2.5"/>'))
add("icons", "globe", "Globe", ["world", "web", "internet", "language", "earth"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="3" y1="12" x2="21" y2="12"/><path d="M12 3 C15 6.5 15 17.5 12 21 C9 17.5 9 6.5 12 3 Z"/>'))
add("icons", "cloud", "Cloud", ["weather", "storage", "upload", "sky"],
    icon('<path d="M7 18 A4 4 0 0 1 7 10 A5 5 0 0 1 17 9 A3.5 3.5 0 0 1 17.5 18 Z"/>'))
add("icons", "download", "Download", ["save", "import", "down", "arrow"],
    icon('<line x1="12" y1="3" x2="12" y2="15"/><polyline points="7,11 12,16 17,11"/><polyline points="4,20 20,20"/>'))
add("icons", "upload", "Upload", ["export", "send", "up", "arrow"],
    icon('<line x1="12" y1="16" x2="12" y2="4"/><polyline points="7,9 12,4 17,9"/><polyline points="4,20 20,20"/>'))
add("icons", "trash", "Trash", ["delete", "remove", "bin", "garbage"],
    icon('<polyline points="4,6 20,6"/><path d="M6 6 V20 H18 V6"/><path d="M9 6 V4 H15 V6"/><line x1="10" y1="10" x2="10" y2="17"/><line x1="14" y1="10" x2="14" y2="17"/>'))
add("icons", "edit", "Edit", ["pencil", "write", "modify", "compose"],
    icon('<path d="M4 20 L4 16 L16 4 L20 8 L8 20 Z"/><line x1="13" y1="7" x2="17" y2="11"/>'))
add("icons", "folder", "Folder", ["directory", "files", "archive"],
    icon('<path d="M3 6 H9 L11 8 H21 V19 H3 Z"/>'))
add("icons", "file", "File", ["document", "page", "paper"],
    icon('<path d="M6 3 H14 L19 8 V21 H6 Z"/><polyline points="14,3 14,8 19,8"/>'))
add("icons", "document", "Document", ["file", "text", "page", "report"],
    icon('<path d="M6 3 H14 L19 8 V21 H6 Z"/><polyline points="14,3 14,8 19,8"/><line x1="9" y1="12" x2="16" y2="12"/><line x1="9" y1="16" x2="16" y2="16"/>'))
add("icons", "eye", "Eye", ["view", "visible", "show", "preview", "watch"],
    icon('<path d="M2 12 C5 6 9 4 12 4 C15 4 19 6 22 12 C19 18 15 20 12 20 C9 20 5 18 2 12 Z"/><circle cx="12" cy="12" r="3"/>'))
add("icons", "eye-off", "Eye Off", ["hide", "hidden", "invisible", "private"],
    icon('<path d="M4 5 C2.8 6.7 2 9 2 12 C5 18 9 20 12 20 C13.7 20 15.6 19.3 17.3 18"/><path d="M9.5 5.2 C10.3 5.1 11.1 5 12 5 C15 5 19 7 22 13"/><line x1="3" y1="3" x2="21" y2="21"/>'))
add("icons", "menu", "Menu", ["hamburger", "list", "navigation", "bars"],
    icon('<line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/>'))
add("icons", "grid", "Grid", ["layout", "apps", "tiles", "dashboard"],
    icon('<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/>'))
add("icons", "play", "Play", ["start", "media", "video", "run"],
    icon('<polygon points="7,4 20,12 7,20" fill="none"/>'))
add("icons", "pause", "Pause", ["stop", "media", "hold"],
    icon('<line x1="9" y1="5" x2="9" y2="19"/><line x1="15" y1="5" x2="15" y2="19"/>'))
add("icons", "wifi", "Wi-Fi", ["network", "signal", "internet", "wireless"],
    icon('<path d="M2 8.5 C8 3.5 16 3.5 22 8.5"/><path d="M5.5 12.5 C9 9.5 15 9.5 18.5 12.5"/><path d="M9 16.5 C10.7 15 13.3 15 15 16.5"/><line x1="12" y1="20" x2="12" y2="20.2"/>'))
add("icons", "sun", "Sun", ["light", "day", "weather", "bright", "theme"],
    icon('<circle cx="12" cy="12" r="4.5"/><path d="M12 2 V4.5 M12 19.5 V22 M2 12 H4.5 M19.5 12 H22 M4.9 4.9 L6.7 6.7 M17.3 17.3 L19.1 19.1 M19.1 4.9 L17.3 6.7 M6.7 17.3 L4.9 19.1"/>'))
add("icons", "moon", "Moon", ["night", "dark", "weather", "sleep", "theme"],
    icon('<path d="M20 14 A8 8 0 1 1 10 4 A6 6 0 0 0 20 14 Z"/>'))
add("icons", "bookmark", "Bookmark", ["save", "tag", "favorite", "ribbon"],
    icon('<path d="M6 3 H18 V21 L12 16 L6 21 Z"/>'))
add("icons", "tag", "Tag", ["label", "price", "category", "sale"],
    icon('<path d="M3 11 V4 H10 L21 15 L14 22 Z"/><circle cx="7.5" cy="7.5" r="1.3"/>'))
add("icons", "link", "Link", ["chain", "url", "connect", "hyperlink"],
    icon('<path d="M9 15 L15 9"/><path d="M11 6 L13 4 A4 4 0 0 1 18 9 L16 11"/><path d="M13 18 L11 20 A4 4 0 0 1 6 15 L8 13"/>'))
add("icons", "filter", "Filter", ["funnel", "sort", "refine"],
    icon('<polygon points="3,5 21,5 14,13 14,20 10,18 10,13"/>'))
add("icons", "refresh", "Refresh", ["reload", "sync", "update", "retry"],
    icon('<path d="M20 12 A8 8 0 1 1 17.5 6.5"/><polyline points="20,4 20,9 15,9"/>'))
add("icons", "power", "Power", ["on", "off", "shutdown", "energy"],
    icon('<path d="M8 5.5 A8 8 0 1 0 16 5.5"/><line x1="12" y1="2" x2="12" y2="11"/>'))
add("icons", "thumbs-up", "Thumbs Up", ["like", "approve", "vote", "good"],
    icon('<path d="M7 10 L11 3 C13 3 14 4.5 13.5 7 L13 10 H19 A2 2 0 0 1 21 12.4 L19.5 19 A2 2 0 0 1 17.5 20.5 H7 Z"/><line x1="7" y1="10" x2="7" y2="20.5"/><rect x="3" y="10" width="4" height="10.5"/>'))
add("icons", "message-circle", "Chat", ["message", "comment", "talk", "support", "bubble"],
    icon('<path d="M4 18 L4 7 A2 2 0 0 1 6 5 H18 A2 2 0 0 1 20 7 V15 A2 2 0 0 1 18 17 H8 Z"/>'))
add("icons", "send", "Send", ["paper-plane", "submit", "share", "message"],
    icon('<polygon points="3,11 21,3 13,21 11,13"/>'))
add("icons", "play-circle", "Play Circle", ["video", "media", "start", "watch"],
    icon('<circle cx="12" cy="12" r="9"/><polygon points="10,8.5 16,12 10,15.5"/>'))
add("icons", "info", "Info", ["help", "about", "details", "information"],
    icon('<circle cx="12" cy="12" r="9"/><line x1="12" y1="11" x2="12" y2="16"/><line x1="12" y1="8" x2="12" y2="8.1"/>'))

# --------------------------------------------------------------------------
# Frames
# --------------------------------------------------------------------------
add("frames", "frame-rectangle", "Rectangle Frame", ["border", "outline", "box"],
    frame('<rect x="2" y="4" width="20" height="16"/>'))
add("frames", "frame-rounded", "Rounded Frame", ["border", "outline", "card"],
    frame('<rect x="2" y="4" width="20" height="16" rx="3"/>'))
add("frames", "frame-circle", "Circle Frame", ["border", "outline", "round", "avatar"],
    frame('<circle cx="12" cy="12" r="10"/>'))
add("frames", "frame-square", "Square Frame", ["border", "outline", "box"],
    frame('<rect x="4" y="4" width="16" height="16"/>'))
add("frames", "frame-double", "Double Frame", ["border", "outline", "nested"],
    frame('<rect x="2" y="3" width="20" height="18"/><rect x="5" y="6" width="14" height="12"/>'))
add("frames", "frame-corners", "Corner Brackets", ["crop", "focus", "marker", "brackets"],
    frame('<polyline points="3,8 3,3 8,3"/><polyline points="16,3 21,3 21,8"/><polyline points="21,16 21,21 16,21"/><polyline points="8,21 3,21 3,16"/>'))
add("frames", "frame-portrait", "Portrait Frame", ["border", "photo", "vertical"],
    frame('<rect x="5" y="2" width="14" height="20" rx="1"/>'))
add("frames", "frame-landscape", "Landscape Frame", ["border", "photo", "horizontal"],
    frame('<rect x="2" y="5" width="20" height="14" rx="1"/>'))
add("frames", "frame-polaroid", "Polaroid Frame", ["photo", "instant", "picture"],
    frame('<rect x="3" y="3" width="18" height="18"/><rect x="5.5" y="5.5" width="13" height="9.5"/>'))
add("frames", "frame-hexagon", "Hexagon Frame", ["border", "outline", "polygon"],
    frame('<polygon points="6,3 18,3 23,12 18,21 6,21 1,12"/>'))

# --------------------------------------------------------------------------
# Flat illustrations (multi-colour)
# --------------------------------------------------------------------------
add("illustrations", "mountain-sun", "Mountain & Sun", ["landscape", "nature", "scene", "outdoor", "hill"],
    shape(
        f'<rect x="1" y="2" width="22" height="20" rx="2" fill="{SKY}"/>'
        f'<circle cx="17" cy="8" r="3" fill="{SUN}"/>'
        f'<path d="M1 22 L8 11 L13 18 L16 14 L23 22 Z" fill="{GRASS}"/>'
        f'<path d="M8 11 L11.2 16 L4.8 16 Z" fill="{ROCK}"/>'
    ))
add("illustrations", "cloud-sun", "Sun & Cloud", ["weather", "sky", "forecast", "day"],
    shape(
        f'<circle cx="9" cy="9" r="4" fill="{SUN}"/>'
        f'<path d="M11 19 A3.5 3.5 0 0 1 11 12 A4.5 4.5 0 0 1 20 11.5 A3 3 0 0 1 20 19 Z" fill="{WHITE}" stroke="{ROCK}" stroke-width="1"/>'
    ))
add("illustrations", "rocket", "Rocket", ["launch", "startup", "space", "boost", "fast"],
    shape(
        f'<path d="M12 2 C16 5 17 10 16 15 H8 C7 10 8 5 12 2 Z" fill="{PAPER}" stroke="{INK}" stroke-width="1"/>'
        f'<circle cx="12" cy="9" r="2" fill="{INDIGO}"/>'
        f'<path d="M8 13 L5 16 L8 15 Z" fill="{CORAL}"/>'
        f'<path d="M16 13 L19 16 L16 15 Z" fill="{CORAL}"/>'
        f'<path d="M10 15 H14 L13 20 L12 18 L11 20 Z" fill="{AMBER}"/>'
    ))
add("illustrations", "lightbulb", "Idea", ["bulb", "light", "innovation", "tip", "think"],
    shape(
        f'<path d="M12 2 A7 7 0 0 1 16 15 H8 A7 7 0 0 1 12 2 Z" fill="{SUN}"/>'
        f'<rect x="9" y="15" width="6" height="3" fill="{ROCK}"/>'
        f'<rect x="10" y="18" width="4" height="3" rx="1" fill="{INK}"/>'
    ))
add("illustrations", "target", "Target", ["goal", "aim", "bullseye", "focus", "objective"],
    shape(
        f'<circle cx="12" cy="12" r="10" fill="{CORAL}"/>'
        f'<circle cx="12" cy="12" r="6.5" fill="{PAPER}"/>'
        f'<circle cx="12" cy="12" r="3" fill="{CORAL}"/>'
    ))
add("illustrations", "chat-bubbles", "Chat Bubbles", ["message", "conversation", "talk", "comment", "support"],
    shape(
        f'<path d="M2 4 H16 A2 2 0 0 1 18 6 V12 A2 2 0 0 1 16 14 H8 L4 17 V14 H2 Z" fill="{INDIGO}"/>'
        f'<path d="M22 10 V17 A2 2 0 0 1 20 19 H12 L9 21.5 V19 H10 A2 2 0 0 1 8 17 V16 H16 A2 2 0 0 0 18 14 H20 A2 2 0 0 1 22 10 Z" fill="{TEAL}"/>'
    ))
add("illustrations", "clipboard-check", "Checklist", ["todo", "task", "list", "done", "clipboard"],
    shape(
        f'<rect x="4" y="3" width="16" height="19" rx="2" fill="{PAPER}" stroke="{ROCK}" stroke-width="1"/>'
        f'<rect x="9" y="2" width="6" height="3" rx="1" fill="{ROCK}"/>'
        f'<polyline points="7,10 9,12 12,8.5" fill="none" stroke="{GRASS}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<polyline points="7,16 9,18 12,14.5" fill="none" stroke="{GRASS}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<line x1="14" y1="11" x2="17" y2="11" stroke="{ROCK}" stroke-width="2" stroke-linecap="round"/>'
        f'<line x1="14" y1="17" x2="17" y2="17" stroke="{ROCK}" stroke-width="2" stroke-linecap="round"/>'
    ))
add("illustrations", "trophy", "Trophy", ["award", "win", "prize", "champion", "winner"],
    shape(
        f'<path d="M7 3 H17 V9 A5 5 0 0 1 7 9 Z" fill="{SUN}"/>'
        f'<path d="M7 5 H4 V7 A3 3 0 0 0 7 10" fill="none" stroke="{AMBER}" stroke-width="1.5"/>'
        f'<path d="M17 5 H20 V7 A3 3 0 0 1 17 10" fill="none" stroke="{AMBER}" stroke-width="1.5"/>'
        f'<rect x="10" y="13" width="4" height="4" fill="{AMBER}"/>'
        f'<rect x="7" y="17" width="10" height="3" rx="1" fill="{ROCK}"/>'
    ))
add("illustrations", "gift", "Gift", ["present", "reward", "box", "surprise", "birthday"],
    shape(
        f'<rect x="3" y="9" width="18" height="12" rx="1" fill="{INDIGO}"/>'
        f'<rect x="3" y="7" width="18" height="4" fill="{TEAL}"/>'
        f'<rect x="10.5" y="7" width="3" height="14" fill="{AMBER}"/>'
        f'<path d="M12 7 C9 7 7 3 9.5 3 C11.5 3 12 7 12 7 Z" fill="{AMBER}"/>'
        f'<path d="M12 7 C15 7 17 3 14.5 3 C12.5 3 12 7 12 7 Z" fill="{AMBER}"/>'
    ))
add("illustrations", "flag", "Flag", ["banner", "milestone", "marker", "country", "goal"],
    shape(
        f'<line x1="5" y1="2" x2="5" y2="22" stroke="{INK}" stroke-width="2" stroke-linecap="round"/>'
        f'<path d="M5 3 H20 L17 7.5 L20 12 H5 Z" fill="{CORAL}"/>'
    ))
add("illustrations", "location-map", "Map", ["location", "navigation", "place", "travel", "route"],
    shape(
        f'<path d="M2 5 L9 3 L15 5 L22 3 V19 L15 21 L9 19 L2 21 Z" fill="{GRASS}"/>'
        f'<path d="M9 3 V19 M15 5 V21" stroke="{WHITE}" stroke-width="1" fill="none"/>'
        f'<path d="M15 8 A3 3 0 0 1 18 11 C18 13.5 15 16 15 16 C15 16 12 13.5 12 11 A3 3 0 0 1 15 8 Z" fill="{CORAL}"/>'
    ))
add("illustrations", "coffee", "Coffee", ["cup", "drink", "break", "cafe", "mug"],
    shape(
        f'<path d="M4 8 H18 V14 A5 5 0 0 1 8 14 Z" fill="{INK}"/>'
        f'<path d="M18 9 H20 A2 2 0 0 1 20 13 H18" fill="none" stroke="{INK}" stroke-width="1.5"/>'
        f'<rect x="6" y="19" width="14" height="2" rx="1" fill="{ROCK}"/>'
        f'<path d="M9 3 C9 4.5 8 4.5 8 6 M13 3 C13 4.5 12 4.5 12 6" fill="none" stroke="{ROCK}" stroke-width="1.2" stroke-linecap="round"/>'
    ))

# --------------------------------------------------------------------------
# Emit SVG files + the Rust catalog table.
# --------------------------------------------------------------------------
ID_RE = re.compile(r"^[a-z0-9-]+$")


def main():
    rows = []
    seen = set()
    total = 0
    for cat, items in ASSETS.items():
        cat_dir = os.path.join(DATA, cat)
        os.makedirs(cat_dir, exist_ok=True)
        for _id, name, tags, svg_text in items:
            assert ID_RE.match(_id), f"bad id {_id!r}"
            key = (cat, _id)
            assert key not in seen, f"duplicate {key}"
            seen.add(key)
            with open(os.path.join(cat_dir, f"{_id}.svg"), "w", encoding="utf-8") as fh:
                fh.write(svg_text)
            tag_lits = ", ".join(f'"{t}"' for t in tags)
            variant = {
                "shapes": "Shapes",
                "lines": "Lines",
                "icons": "Icons",
                "frames": "Frames",
                "illustrations": "Illustrations",
            }[cat]
            rows.append(
                f'    AssetDef {{\n'
                f'        id: "{_id}",\n'
                f'        name: "{name}",\n'
                f'        category: AssetCategory::{variant},\n'
                f'        tags: &[{tag_lits}],\n'
                f'        svg: include_str!("data/{cat}/{_id}.svg"),\n'
                f'    }},'
            )
            total += 1

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
