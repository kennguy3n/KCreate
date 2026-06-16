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
  "welcome.phase.resolving": "Resolviendo la recomendación…",
  "welcome.phase.connecting": "Conectando…",
  "welcome.phase.downloading": "Descargando…",
  "welcome.phase.verifying": "Verificando…",
  "welcome.phase.installing": "Instalando…",
  "welcome.phase.done": "Hecho",
  "welcome.phase.cancelled": "Cancelado",
  "welcome.phase.error": "Error",

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

  // Language switcher.
  "lang.label": "Idioma",
  "lang.aria": "Cambiar idioma",
  "lang.changed": "Idioma cambiado a {language}",
};
