import type { PartialMessages } from "../types";

// Spanish (es) catalog. Covers the core surfaces — app shell, top bar,
// command palette, onboarding modal, and home page. Any key omitted
// here falls back to the English value at format time.
export const es: PartialMessages = {
  // App shell / routing.
  "app.editor.loading": "Cargando el editor…",
  "app.editor.loadFailed.title": "No se pudo cargar el editor",
  "app.action.reload": "Recargar",
  "app.action.backToHome": "Volver al inicio",
  "app.error.openProject": "No se pudo abrir el proyecto: {message}",
  "app.error.briefProjectClosed":
    "Se aplicó el resumen, pero el proyecto se cerró antes de que el editor pudiera abrirlo.",

  // Top bar.
  "topbar.home": "Inicio",
  "topbar.search": "Buscar",
  "topbar.search.hint": "Buscar acciones, paneles y herramientas",
  "topbar.templates": "Plantillas",
  "topbar.templates.hint": "Empezar desde una plantilla",
  "topbar.generate": "Generar",
  "topbar.generate.hint": "Generar un diseño temático con IA",
  "topbar.export": "Exportar",
  "topbar.aria.backToHome": "Volver al inicio",
  "topbar.aria.openCommandPalette": "Abrir la paleta de comandos",
  "topbar.aria.editorMode": "Modo del editor",
  "topbar.aria.drawingTools": "Herramientas de dibujo",
  "topbar.aria.browseTemplates": "Explorar plantillas",
  "topbar.aria.generateWithAi": "Generar con IA",
  "topbar.aria.undo": "Deshacer",
  "topbar.aria.redo": "Rehacer",
  "topbar.aria.switchToLight": "Cambiar al tema claro",
  "topbar.aria.switchToDark": "Cambiar al tema oscuro",
  "topbar.theme.dark": "Tema: Oscuro",
  "topbar.theme.light": "Tema: Claro",
  "topbar.tool.title": "{label} ({key})",

  // Editor modes.
  "topbar.mode.design": "Diseño",
  "topbar.mode.vector": "Vector",
  "topbar.mode.image": "Imagen",
  "topbar.mode.layout": "Maquetación",
  "topbar.mode.prototype": "Prototipo",
  "topbar.mode.inspect": "Inspeccionar",
  "topbar.mode.export": "Exportar",

  // Drawing tools.
  "topbar.tool.select": "Seleccionar",
  "topbar.tool.rect": "Rectángulo",
  "topbar.tool.ellipse": "Elipse",
  "topbar.tool.line": "Línea",
  "topbar.tool.pen": "Pluma",
  "topbar.tool.text": "Texto",

  // Command palette.
  "palette.aria.dialog": "Paleta de comandos",
  "palette.placeholder": "Buscar acciones, paneles, herramientas…",
  "palette.aria.searchInput": "Buscar comandos",
  "palette.esc": "Esc",
  "palette.empty": "No hay comandos que coincidan.",
  "palette.recent": "Recientes",
  "palette.footer.navigate": "navegar",
  "palette.footer.run": "ejecutar",
  "palette.footer.dismiss": "cerrar",

  // Conjunto de comandos de la paleta (se construye en EditorPage).
  "palette.group.create": "Crear",
  "palette.group.panels": "Paneles",
  "palette.group.tools": "Herramientas",
  "palette.group.studios": "Estudios",
  "palette.group.edit": "Editar",
  "palette.group.view": "Ver",
  "palette.cmd.magicResize": "Cambio de tamaño mágico",
  "palette.cmd.openTheme": "Abrir Tema y kit de marca",
  "palette.cmd.openExport": "Exportar",
  "palette.cmd.shortcuts": "Atajos de teclado",
  "palette.cmd.undo": "Deshacer",
  "palette.cmd.redo": "Rehacer",
  "palette.cmd.selectAll": "Seleccionar todo",
  "palette.cmd.copy": "Copiar",
  "palette.cmd.paste": "Pegar",
  "palette.cmd.deleteSelection": "Eliminar la selección",
  "palette.cmd.zoomToFit": "Ajustar a la vista",
  "palette.cmd.backHome": "Volver al inicio",
  "palette.tool.label": "Herramienta {name}",
  "palette.studio.label": "Estudio de {name}",
  "palette.disabled.createArtboard": "Primero crea una mesa de trabajo",
  "palette.disabled.nothingToUndo": "Nada que deshacer",
  "palette.disabled.nothingToRedo": "Nada que rehacer",
  "palette.disabled.nothingSelected": "No hay nada seleccionado",

  // Welcome / onboarding modal.
  "welcome.title": "Te damos la bienvenida a KCreate",
  "welcome.aria.close": "Cerrar la bienvenida",
  "welcome.lead":
    "KCreate se ejecuta por completo en tu dispositivo. Instala ahora un modelo de IA local para activar sugerencias de diseño, nombres de capas y comandos inteligentes, o sáltalo por ahora y elige uno más tarde en el Administrador de modelos.",
  "welcome.loading": "Detectando tu dispositivo…",
  "welcome.alreadyInstalled":
    "Ya tienes este paquete instalado. Todo listo.",
  "welcome.skip": "Saltar por ahora",
  "welcome.pickFile": "Ya tengo el archivo…",
  "welcome.install": "Instalar el paquete recomendado",
  "welcome.cancel": "Cancelar",
  "welcome.finish": "Empezar",
  "welcome.errorDismiss": "Cerrar",
  "welcome.starting": "Iniciando…",
  "welcome.progress.of": "{received} de {total}",
  "welcome.pack.aria": "Paquete recomendado",
  "welcome.pack.tier": "Nivel {tier}",
  "welcome.pack.desc":
    "GGUF cuantizado, se ejecuta en tu dispositivo mediante llama.cpp. Ningún dato sale de tu máquina.",
  "welcome.ready.suffix": "está listo.",
  "welcome.verified": "Verificado {size}.",
  "welcome.unverified":
    "Instalado {size} (sin SHA-256 fijado en el registro; hash real {hash}…).",
  "welcome.error.noRecommendedPack":
    "El nivel de tu dispositivo aún no tiene un paquete de LLM local recomendado. Aún puedes instalar un paquete manualmente desde el Gestor de modelos.",
  "welcome.error.packNotInRegistry":
    "El paquete recomendado '{packId}' no está en el registro de modelos. Abre el Gestor de modelos para instalar un paquete manualmente.",
  "welcome.phase.resolving": "Resolviendo la recomendación…",
  "welcome.phase.connecting": "Conectando…",
  "welcome.phase.downloading": "Descargando…",
  "welcome.phase.verifying": "Verificando…",
  "welcome.phase.installing": "Instalando…",
  "welcome.phase.done": "Hecho",
  "welcome.phase.cancelled": "Cancelado",
  "welcome.phase.error": "Error",

  // Superposición de descubrimiento (primer arranque del editor).
  "discovery.title": "Te damos la bienvenida a KCreate",
  "discovery.lead":
    "Todo está a una tecla de distancia. Abre la paleta de comandos para saltar a cualquier herramienta, panel o flujo.",
  "discovery.aria.close": "Descartar la bienvenida",
  "discovery.openPalette": "Abrir la paleta de comandos",
  "discovery.skip": "Quizá más tarde",

  // Texto compartido de los flujos de creación.
  "create.templates.label": "Empezar desde una plantilla",
  "create.templates.desc": "Bifurca un diseño listo para usar y hazlo tuyo.",
  "create.ai.label": "Generar con IA",
  "create.ai.desc": "Descríbelo y deja que el modelo local lo redacte.",
  "create.elements.label": "Explorar elementos",
  "create.elements.desc": "Añade formas, iconos e ilustraciones.",

  // Llamada a la acción del lienzo vacío.
  "canvasEmpty.title": "Empieza tu primer diseño",
  "canvasEmpty.lead":
    "Elige una plantilla lista para usar, descríbela a la IA o explora la biblioteca de elementos, o pulsa {hint} para todo.",
  "canvasEmpty.openPalette": "Abrir la paleta de comandos",

  // Home page sections.
  "home.section.startFromTemplate": "Empezar desde una plantilla",
  "home.section.startFromBrief": "Empezar desde un resumen",
  "home.section.createNew": "Crear nuevo",
  "home.section.recentProjects": "Proyectos recientes",
  "home.section.modelStatus": "Estado del modelo",
  "home.section.helpAndLearn": "Ayuda y aprendizaje",

  // Brief / template entry tiles.
  "home.brief.title": "Empezar desde un resumen",
  "home.brief.blurb.ready":
    "Describe lo que quieres; genera una presentación temática de varias páginas o una de una sola página, o deja que el modelo local complete una sola mesa de trabajo.",
  "home.brief.blurb.offline":
    "Describe lo que quieres y genera una presentación temática de varias páginas o de una sola página: funciona sin conexión.",
  "home.template.title": "Explorar plantillas listas para usar",
  "home.template.blurb":
    "Elige un punto de partida diseñado profesionalmente —presentaciones, publicaciones sociales, kits de UI móvil, carteles, currículums— y empieza directamente en un lienzo con contenido.",

  // Create-new cards.
  "home.create.app-ui.title": "UI de app / sitio web",
  "home.create.app-ui.blurb": "Marcos, componentes, tokens de diseño",
  "home.create.brand.title": "Logo / icono / kit de marca",
  "home.create.brand.blurb": "Marcas vectoriales, paletas, tipografía",
  "home.create.social.title": "Publicación para redes sociales",
  "home.create.social.blurb": "Tamaños habituales para cada canal",
  "home.create.photo.title": "Retoque de foto de producto",
  "home.create.photo.blurb": "Eliminación de fondo, retoque",
  "home.create.deck.title": "Presentación / propuesta",
  "home.create.deck.blurb": "Diseños de varias páginas, páginas maestras",
  "home.create.print.title": "Folleto / cartel / tríptico",
  "home.create.print.blurb": "PDF listo para imprimir, CMYK, sangrado",
  "home.create.dev-export.title": "Exportación de recursos para desarrollo",
  "home.create.dev-export.blurb": "Iconos, SVG, PNG, fragmentos de código",
  "home.create.import.title": "Importar un archivo existente",
  "home.create.import.blurb": "SVG, PNG, JPEG, PDF",

  // Model-status cards.
  "home.model.deviceTier": "Nivel del dispositivo",
  "home.model.gpuBackend": "Backend de GPU",
  "home.model.systemRam": "RAM del sistema",
  "home.model.llmSidecar": "Servicio LLM",
  "home.model.cpuOnly": "Solo CPU",
  "home.model.ramMb": "{mb} MB",

  // Help & learn links.
  "home.help.gettingStarted.label": "Primeros pasos",
  "home.help.gettingStarted.blurb":
    "Recorrido inicial: mesas de trabajo, capas, exportación.",
  "home.help.shortcuts.label": "Atajos de teclado",
  "home.help.shortcuts.blurb":
    "Todos los atajos en un solo lugar — hoja de referencia imprimible.",
  "home.help.whatsNew.label": "Novedades",
  "home.help.whatsNew.blurb": "Registro de cambios y funciones destacadas.",
  "home.help.architecture.label": "Arquitectura",
  "home.help.architecture.blurb":
    "Local primero, Rust + Electron, documentación técnica detallada.",

  // Recent-projects grid states.
  "home.recents.loading": "Cargando proyectos recientes…",
  "home.recents.error": "No se pudo leer la lista de proyectos recientes:",
  "home.recents.empty":
    "Aún no hay proyectos recientes. Tu trabajo se guarda localmente en carpetas .kstudio; empieza desde una plantilla lista para usar y consigue un diseño real en el lienzo con un solo clic.",
  "home.recents.browseTemplates": "Explorar plantillas",
  "home.recents.noPreview": "sin vista previa",
  "home.runtime.probeFailed": "falló la comprobación del entorno: {error}",
  "home.runtime.cpuOnly": "Solo CPU",

  // Editor status bar.
  "editor.status.project": "Proyecto: {path}",
  "editor.status.noSelection": "Sin selección",
  "editor.status.selected":
    "{count, plural, one {# seleccionado} other {# seleccionados}}",

  // Cadenas varias del editor.
  "editor.magicResize.needsArtboard":
    "El cambio de tamaño mágico necesita una mesa de trabajo: crea una primero.",
  "editor.preview.play": "Reproducir",
  "editor.import.dropHint":
    "Suelta archivos para importar (PNG, JPG, WebP, GIF, SVG, PDF)",
  "editor.shortcuts.title": "Atajos de teclado",

  // Marca temporal relativa en las tarjetas de proyectos recientes.
  "home.recents.justNow": "ahora mismo",

  // Language switcher.
  "lang.label": "Idioma",
  "lang.aria": "Cambiar idioma",
  "lang.changed": "Idioma cambiado a {language}",
};
