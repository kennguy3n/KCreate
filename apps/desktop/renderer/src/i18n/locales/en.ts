// English message catalog — the source of truth for KCreate's UI
// strings. Every other locale is a `Partial` of this catalog (see
// `../types.ts`); any key a translation omits falls back to the
// English value here at format time, so shipping a partial locale can
// never produce a blank label.
//
// Keys are dot-namespaced by surface (`topbar.*`, `home.*`, …) and the
// object is frozen with `as const` so `keyof typeof en` yields the
// exhaustive `MessageKey` union the rest of the layer is typed against.
// Values use the ICU-lite grammar understood by `../format.ts`
// (`{name}` interpolation, `{n, plural, …}` pluralization).

export const en = {
  // App shell / routing.
  "app.editor.loading": "Loading editor…",
  "app.editor.loadFailed.title": "The editor failed to load",
  "app.action.reload": "Reload",
  "app.action.backToHome": "Back to home",
  "app.error.openProject": "Failed to open project: {message}",
  "app.error.briefProjectClosed":
    "Brief applied but the project was closed before the editor could open it.",

  // Top bar.
  "topbar.home": "Home",
  "topbar.search": "Search",
  "topbar.search.hint": "Search actions, panels, and tools",
  "topbar.templates": "Templates",
  "topbar.templates.hint": "Start from a template",
  "topbar.generate": "Generate",
  "topbar.generate.hint": "Generate a themed design with AI",
  "topbar.export": "Export",
  "topbar.aria.backToHome": "Back to home",
  "topbar.aria.openCommandPalette": "Open command palette",
  "topbar.aria.editorMode": "Editor mode",
  "topbar.aria.drawingTools": "Drawing tools",
  "topbar.aria.browseTemplates": "Browse templates",
  "topbar.aria.generateWithAi": "Generate with AI",
  "topbar.aria.undo": "Undo",
  "topbar.aria.redo": "Redo",
  "topbar.aria.switchToLight": "Switch to light theme",
  "topbar.aria.switchToDark": "Switch to dark theme",
  "topbar.theme.dark": "Theme: Dark",
  "topbar.theme.light": "Theme: Light",
  "topbar.tool.title": "{label} ({key})",

  // Editor modes (mirror EDITOR_MODES labels).
  "topbar.mode.design": "Design",
  "topbar.mode.vector": "Vector",
  "topbar.mode.image": "Image",
  "topbar.mode.layout": "Layout",
  "topbar.mode.prototype": "Prototype",
  "topbar.mode.inspect": "Inspect",
  "topbar.mode.export": "Export",

  // Drawing tools (label must contain the tool id for the a11y name).
  "topbar.tool.select": "Select",
  "topbar.tool.rect": "Rect",
  "topbar.tool.ellipse": "Ellipse",
  "topbar.tool.line": "Line",
  "topbar.tool.pen": "Pen",
  "topbar.tool.text": "Text",

  // Command palette.
  "palette.aria.dialog": "Command palette",
  "palette.placeholder": "Search actions, panels, tools…",
  "palette.aria.searchInput": "Search commands",
  "palette.esc": "Esc",
  "palette.empty": "No matching commands.",
  "palette.recent": "Recent",
  "palette.footer.navigate": "navigate",
  "palette.footer.run": "run",
  "palette.footer.dismiss": "dismiss",

  // Command-palette command set (built in EditorPage). Group headers,
  // command labels, the tool/studio name templates, and the
  // disabled-state reasons all flow through here.
  "palette.group.create": "Create",
  "palette.group.panels": "Panels",
  "palette.group.tools": "Tools",
  "palette.group.studios": "Studios",
  "palette.group.edit": "Edit",
  "palette.group.view": "View",
  "palette.cmd.magicResize": "Magic resize",
  "palette.cmd.openTheme": "Open Theme & Brand kit",
  "palette.cmd.openExport": "Export",
  "palette.cmd.shortcuts": "Keyboard shortcuts",
  "palette.cmd.undo": "Undo",
  "palette.cmd.redo": "Redo",
  "palette.cmd.selectAll": "Select all",
  "palette.cmd.copy": "Copy",
  "palette.cmd.paste": "Paste",
  "palette.cmd.deleteSelection": "Delete selection",
  "palette.cmd.zoomToFit": "Zoom to fit",
  "palette.cmd.backHome": "Back to Home",
  // `{name}` is the localized tool/mode name (see topbar.tool.* / topbar.mode.*).
  "palette.tool.label": "{name} tool",
  "palette.studio.label": "{name} studio",
  "palette.disabled.createArtboard": "Create an artboard first",
  "palette.disabled.nothingToUndo": "Nothing to undo",
  "palette.disabled.nothingToRedo": "Nothing to redo",
  "palette.disabled.nothingSelected": "Nothing selected",

  // Welcome / onboarding modal.
  "welcome.title": "Welcome to KCreate",
  "welcome.aria.close": "Close welcome",
  "welcome.lead":
    "KCreate runs entirely on your device. Install a local AI model now to enable design suggestions, layer naming, and smart commands — or skip for now and pick one from the Model Manager later.",
  "welcome.loading": "Detecting your device…",
  "welcome.alreadyInstalled":
    "You already have this pack installed. You’re good to go.",
  "welcome.skip": "Skip for now",
  "welcome.pickFile": "I already have the file…",
  "welcome.install": "Install recommended pack",
  "welcome.cancel": "Cancel",
  "welcome.finish": "Get started",
  "welcome.errorDismiss": "Close",
  "welcome.starting": "Starting…",
  "welcome.progress.of": "{received} of {total}",
  "welcome.pack.aria": "Recommended pack",
  "welcome.pack.tier": "Tier {tier}",
  "welcome.pack.desc":
    "Quantised GGUF, runs on your device via llama.cpp. No data leaves your machine.",
  "welcome.ready.suffix": "is ready.",
  "welcome.verified": "Verified {size}.",
  "welcome.unverified":
    "Installed {size} (no pinned SHA-256 in the registry; actual hash {hash}…).",
  "welcome.error.noRecommendedPack":
    "Your device tier does not have a recommended local LLM pack yet. You can still install a pack manually from Model Manager.",
  "welcome.error.packNotInRegistry":
    "Recommended pack '{packId}' is not in the model registry. Open Model Manager to install a pack manually.",
  "welcome.phase.resolving": "Resolving recommendation…",
  "welcome.phase.connecting": "Connecting…",
  "welcome.phase.downloading": "Downloading…",
  "welcome.phase.verifying": "Verifying…",
  "welcome.phase.installing": "Installing…",
  "welcome.phase.done": "Done",
  "welcome.phase.cancelled": "Cancelled",
  "welcome.phase.error": "Error",

  // First-run discovery overlay (editor). Separate from the
  // bridge-backed welcome modal above.
  "discovery.title": "Welcome to KCreate",
  "discovery.lead":
    "Everything is one keystroke away. Press the command palette to jump to any tool, panel, or flow.",
  "discovery.aria.close": "Dismiss welcome",
  "discovery.openPalette": "Open the command palette",
  "discovery.skip": "Maybe later",

  // Shared create-flow copy — the three headline flows surfaced by the
  // discovery cards, the empty-canvas state, and the command palette,
  // kept on one set of keys so each reads identically everywhere.
  "create.templates.label": "Start from a template",
  "create.templates.desc": "Fork a ready-made design and make it yours.",
  "create.ai.label": "Generate with AI",
  "create.ai.desc": "Describe it and let the local model draft it.",
  "create.elements.label": "Browse elements",
  "create.elements.desc": "Drop in shapes, icons, and illustrations.",

  // Home page sections.
  "home.section.startFromTemplate": "Start from a template",
  "home.section.startFromBrief": "Start from a brief",
  "home.section.createNew": "Create new",
  "home.section.recentProjects": "Recent projects",
  "home.section.modelStatus": "Model status",
  "home.section.helpAndLearn": "Help & learn",

  // Brief / template entry tiles.
  "home.brief.title": "Start from a brief",
  "home.brief.blurb.ready":
    "Describe what you want; generate a themed multi-page deck or one-pager, or let the local model fill a single artboard.",
  "home.brief.blurb.offline":
    "Describe what you want and generate a themed multi-page deck or one-pager — works offline.",
  "home.template.title": "Browse ready-made templates",
  "home.template.blurb":
    "Pick a professionally-designed starter — decks, social posts, mobile UI kits, posters, resumes — and jump straight onto a populated canvas.",

  // Create-new cards (titles mirror CREATE_OPTIONS).
  "home.create.app-ui.title": "App / Website UI",
  "home.create.app-ui.blurb": "Frames, components, design tokens",
  "home.create.brand.title": "Logo / Icon / Brand Kit",
  "home.create.brand.blurb": "Vector marks, palettes, type",
  "home.create.social.title": "Social Media Post",
  "home.create.social.blurb": "Common sizes for every channel",
  "home.create.photo.title": "Product Photo Cleanup",
  "home.create.photo.blurb": "Background removal, retouching",
  "home.create.deck.title": "Pitch Deck / Proposal",
  "home.create.deck.blurb": "Multi-page layouts, master pages",
  "home.create.print.title": "Flyer / Poster / Brochure",
  "home.create.print.blurb": "Print-ready PDF, CMYK, bleed",
  "home.create.dev-export.title": "Developer Asset Export",
  "home.create.dev-export.blurb": "Icons, SVG, PNG, code snippets",
  "home.create.import.title": "Import Existing File",
  "home.create.import.blurb": "SVG, PNG, JPEG, PDF",

  // Model-status cards.
  "home.model.deviceTier": "Device tier",
  "home.model.gpuBackend": "GPU backend",
  "home.model.systemRam": "System RAM",
  "home.model.llmSidecar": "LLM sidecar",
  "home.model.cpuOnly": "CPU only",
  "home.model.ramMb": "{mb} MB",

  // Help & learn links.
  "home.help.gettingStarted.label": "Getting started",
  "home.help.gettingStarted.blurb":
    "First-run walkthrough: artboards, layers, exporting.",
  "home.help.shortcuts.label": "Keyboard shortcuts",
  "home.help.shortcuts.blurb":
    "Every shortcut in one place — printable cheat sheet.",
  "home.help.whatsNew.label": "What's new",
  "home.help.whatsNew.blurb": "Changelog and feature highlights.",
  "home.help.architecture.label": "Architecture",
  "home.help.architecture.blurb":
    "Local-first, Rust + Electron, deep technical docs.",

  // Recent-projects grid states.
  "home.recents.loading": "Loading recent projects…",
  "home.recents.error": "Could not read the recent-projects list:",
  "home.recents.empty":
    "No recent projects yet. Your work is saved locally inside .kstudio folders — start from a ready-made template to get a real design on the canvas in one click.",
  "home.recents.browseTemplates": "Browse templates",
  "home.recents.noPreview": "no preview",
  "home.runtime.probeFailed": "runtime probe failed: {error}",
  "home.runtime.cpuOnly": "CPU only",

  // Editor status bar (a live region announces save / selection state).
  "editor.status.project": "Project: {path}",
  "editor.status.noSelection": "No selection",
  "editor.status.selected":
    "{count, plural, one {# selected} other {# selected}}",

  // Language switcher.
  "lang.label": "Language",
  "lang.aria": "Change language",
  "lang.changed": "Language changed to {language}",
} as const;
