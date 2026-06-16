import type { PartialMessages } from "../types";

// Arabic (ar) catalog — the RTL locale. Selecting it flips the whole
// shell to `dir="rtl"` (see ../LocaleProvider.tsx). Any key omitted
// here falls back to the English value at format time.
export const ar: PartialMessages = {
  // App shell / routing.
  "app.editor.loading": "جارٍ تحميل المحرّر…",
  "app.editor.loadFailed.title": "تعذّر تحميل المحرّر",
  "app.action.reload": "إعادة التحميل",
  "app.action.backToHome": "العودة إلى الرئيسية",
  "app.error.openProject": "تعذّر فتح المشروع: {message}",
  "app.error.briefProjectClosed":
    "تم تطبيق الموجز، لكن أُغلق المشروع قبل أن يتمكّن المحرّر من فتحه.",

  // Top bar.
  "topbar.home": "الرئيسية",
  "topbar.search": "بحث",
  "topbar.search.hint": "ابحث عن الإجراءات واللوحات والأدوات",
  "topbar.templates": "القوالب",
  "topbar.templates.hint": "ابدأ من قالب",
  "topbar.generate": "إنشاء",
  "topbar.generate.hint": "أنشئ تصميمًا بطابع مميّز باستخدام الذكاء الاصطناعي",
  "topbar.export": "تصدير",
  "topbar.aria.backToHome": "العودة إلى الرئيسية",
  "topbar.aria.openCommandPalette": "فتح لوحة الأوامر",
  "topbar.aria.editorMode": "وضع المحرّر",
  "topbar.aria.drawingTools": "أدوات الرسم",
  "topbar.aria.browseTemplates": "تصفّح القوالب",
  "topbar.aria.generateWithAi": "إنشاء بالذكاء الاصطناعي",
  "topbar.aria.undo": "تراجع",
  "topbar.aria.redo": "إعادة",
  "topbar.aria.switchToLight": "التبديل إلى السمة الفاتحة",
  "topbar.aria.switchToDark": "التبديل إلى السمة الداكنة",
  "topbar.theme.dark": "السمة: داكنة",
  "topbar.theme.light": "السمة: فاتحة",
  "topbar.tool.title": "{label} ({key})",

  // Editor modes.
  "topbar.mode.design": "تصميم",
  "topbar.mode.vector": "متجه",
  "topbar.mode.image": "صورة",
  "topbar.mode.layout": "تخطيط",
  "topbar.mode.prototype": "نموذج أوّلي",
  "topbar.mode.inspect": "فحص",
  "topbar.mode.export": "تصدير",

  // Drawing tools.
  "topbar.tool.select": "تحديد",
  "topbar.tool.rect": "مستطيل",
  "topbar.tool.ellipse": "بيضاوي",
  "topbar.tool.line": "خط",
  "topbar.tool.pen": "قلم",
  "topbar.tool.text": "نص",

  // Command palette.
  "palette.aria.dialog": "لوحة الأوامر",
  "palette.placeholder": "ابحث عن الإجراءات واللوحات والأدوات…",
  "palette.aria.searchInput": "البحث في الأوامر",
  "palette.esc": "Esc",
  "palette.empty": "لا توجد أوامر مطابقة.",
  "palette.recent": "الأخيرة",
  "palette.footer.navigate": "تنقّل",
  "palette.footer.run": "تشغيل",
  "palette.footer.dismiss": "إغلاق",

  // مجموعة أوامر لوحة الأوامر (تُبنى في EditorPage).
  "palette.group.create": "إنشاء",
  "palette.group.panels": "اللوحات",
  "palette.group.tools": "الأدوات",
  "palette.group.studios": "الاستوديوهات",
  "palette.group.edit": "تحرير",
  "palette.group.view": "عرض",
  "palette.cmd.magicResize": "تغيير الحجم السحري",
  "palette.cmd.openTheme": "فتح السمة وطقم العلامة",
  "palette.cmd.openExport": "تصدير",
  "palette.cmd.shortcuts": "اختصارات لوحة المفاتيح",
  "palette.cmd.undo": "تراجع",
  "palette.cmd.redo": "إعادة",
  "palette.cmd.selectAll": "تحديد الكل",
  "palette.cmd.copy": "نسخ",
  "palette.cmd.paste": "لصق",
  "palette.cmd.deleteSelection": "حذف التحديد",
  "palette.cmd.zoomToFit": "ملاءمة العرض",
  "palette.cmd.backHome": "العودة إلى الرئيسية",
  "palette.tool.label": "أداة {name}",
  "palette.studio.label": "استوديو {name}",
  "palette.disabled.createArtboard": "أنشئ لوح رسم أولًا",
  "palette.disabled.nothingToUndo": "لا شيء للتراجع عنه",
  "palette.disabled.nothingToRedo": "لا شيء لإعادته",
  "palette.disabled.nothingSelected": "لا يوجد تحديد",

  // Welcome / onboarding modal.
  "welcome.title": "مرحبًا بك في KCreate",
  "welcome.aria.close": "إغلاق الترحيب",
  "welcome.lead":
    "يعمل KCreate بالكامل على جهازك. ثبّت نموذج ذكاء اصطناعي محلّيًا الآن لتفعيل اقتراحات التصميم وتسمية الطبقات والأوامر الذكية، أو تخطَّ ذلك الآن واختر نموذجًا لاحقًا من مدير النماذج.",
  "welcome.loading": "جارٍ التعرّف على جهازك…",
  "welcome.alreadyInstalled": "هذه الحزمة مثبّتة لديك بالفعل. كل شيء جاهز.",
  "welcome.skip": "تخطَّ الآن",
  "welcome.pickFile": "لديّ الملف بالفعل…",
  "welcome.install": "تثبيت الحزمة الموصى بها",
  "welcome.cancel": "إلغاء",
  "welcome.finish": "ابدأ",
  "welcome.errorDismiss": "إغلاق",
  "welcome.starting": "جارٍ البدء…",
  "welcome.progress.of": "{received} من {total}",
  "welcome.pack.aria": "الحزمة الموصى بها",
  "welcome.pack.tier": "المستوى {tier}",
  "welcome.pack.desc":
    "GGUF مضغوط، يعمل على جهازك عبر llama.cpp. لا تغادر أي بيانات جهازك.",
  "welcome.ready.suffix": "جاهز.",
  "welcome.verified": "تم التحقّق {size}.",
  "welcome.unverified":
    "تم التثبيت {size} (لا يوجد SHA-256 مثبّت في السجل؛ التجزئة الفعلية {hash}…).",
  "welcome.phase.resolving": "جارٍ تحديد التوصية…",
  "welcome.phase.connecting": "جارٍ الاتصال…",
  "welcome.phase.downloading": "جارٍ التنزيل…",
  "welcome.phase.verifying": "جارٍ التحقّق…",
  "welcome.phase.installing": "جارٍ التثبيت…",
  "welcome.phase.done": "تم",
  "welcome.phase.cancelled": "أُلغي",
  "welcome.phase.error": "خطأ",

  // طبقة الاستكشاف عند أول تشغيل للمحرّر.
  "discovery.title": "مرحبًا بك في KCreate",
  "discovery.lead":
    "كل شيء على بُعد ضغطة مفتاح. افتح لوحة الأوامر للانتقال إلى أي أداة أو لوحة أو مسار.",
  "discovery.aria.close": "تجاهل الترحيب",
  "discovery.openPalette": "فتح لوحة الأوامر",
  "discovery.skip": "ربما لاحقًا",

  // نص مشترك لمسارات الإنشاء.
  "create.templates.label": "ابدأ من قالب",
  "create.templates.desc": "انسخ تصميمًا جاهزًا واجعله خاصًا بك.",
  "create.ai.label": "إنشاء بالذكاء الاصطناعي",
  "create.ai.desc": "صِفه ودع النموذج المحلّي يصوغه.",
  "create.elements.label": "تصفّح العناصر",
  "create.elements.desc": "أضِف أشكالًا وأيقونات ورسومًا توضيحية.",

  // Home page sections.
  "home.section.startFromTemplate": "ابدأ من قالب",
  "home.section.startFromBrief": "ابدأ من موجز",
  "home.section.createNew": "إنشاء جديد",
  "home.section.recentProjects": "المشاريع الأخيرة",
  "home.section.modelStatus": "حالة النموذج",
  "home.section.helpAndLearn": "المساعدة والتعلّم",

  // Brief / template entry tiles.
  "home.brief.title": "ابدأ من موجز",
  "home.brief.blurb.ready":
    "صِف ما تريده؛ أنشئ عرضًا تقديميًا متعدّد الصفحات بطابع مميّز أو صفحة واحدة، أو دع النموذج المحلّي يملأ لوح رسم واحدًا.",
  "home.brief.blurb.offline":
    "صِف ما تريده وأنشئ عرضًا تقديميًا متعدّد الصفحات بطابع مميّز أو صفحة واحدة — يعمل دون اتصال.",
  "home.template.title": "تصفّح القوالب الجاهزة",
  "home.template.blurb":
    "اختر نقطة بداية مصمّمة باحتراف — عروض تقديمية ومنشورات اجتماعية وأطقم واجهات للهاتف وملصقات وسير ذاتية — وانتقل مباشرة إلى لوح رسم ممتلئ.",

  // Create-new cards.
  "home.create.app-ui.title": "واجهة تطبيق / موقع ويب",
  "home.create.app-ui.blurb": "إطارات ومكوّنات ورموز تصميم",
  "home.create.brand.title": "شعار / أيقونة / طقم علامة تجارية",
  "home.create.brand.blurb": "علامات متجهة ولوحات ألوان وخطوط",
  "home.create.social.title": "منشور لوسائل التواصل",
  "home.create.social.blurb": "أحجام شائعة لكل قناة",
  "home.create.photo.title": "تنظيف صورة منتج",
  "home.create.photo.blurb": "إزالة الخلفية وتنقيح الصورة",
  "home.create.deck.title": "عرض تقديمي / مقترح",
  "home.create.deck.blurb": "تخطيطات متعدّدة الصفحات وصفحات رئيسية",
  "home.create.print.title": "منشور / ملصق / كتيّب",
  "home.create.print.blurb": "PDF جاهز للطباعة، CMYK، هامش اقتطاع",
  "home.create.dev-export.title": "تصدير أصول للمطوّرين",
  "home.create.dev-export.blurb": "أيقونات، SVG، PNG، مقتطفات شيفرة",
  "home.create.import.title": "استيراد ملف موجود",
  "home.create.import.blurb": "SVG، PNG، JPEG، PDF",

  // Model-status cards.
  "home.model.deviceTier": "فئة الجهاز",
  "home.model.gpuBackend": "خلفية وحدة معالجة الرسومات",
  "home.model.systemRam": "ذاكرة النظام",
  "home.model.llmSidecar": "خدمة LLM",
  "home.model.cpuOnly": "المعالج فقط",
  "home.model.ramMb": "{mb} ميجابايت",

  // Help & learn links.
  "home.help.gettingStarted.label": "البدء",
  "home.help.gettingStarted.blurb":
    "جولة أول تشغيل: ألواح الرسم والطبقات والتصدير.",
  "home.help.shortcuts.label": "اختصارات لوحة المفاتيح",
  "home.help.shortcuts.blurb": "كل الاختصارات في مكان واحد — ورقة مرجعية قابلة للطباعة.",
  "home.help.whatsNew.label": "الجديد",
  "home.help.whatsNew.blurb": "سجلّ التغييرات وأبرز الميزات.",
  "home.help.architecture.label": "البنية",
  "home.help.architecture.blurb":
    "محلّي أولًا، Rust + Electron، وثائق تقنية معمّقة.",

  // Recent-projects grid states.
  "home.recents.loading": "جارٍ تحميل المشاريع الأخيرة…",
  "home.recents.error": "تعذّرت قراءة قائمة المشاريع الأخيرة:",
  "home.recents.empty":
    "لا توجد مشاريع أخيرة بعد. يُحفظ عملك محلّيًا داخل مجلّدات ‎.kstudio‎ — ابدأ من قالب جاهز لتحصل على تصميم حقيقي على لوح الرسم بنقرة واحدة.",
  "home.recents.browseTemplates": "تصفّح القوالب",
  "home.recents.noPreview": "لا توجد معاينة",
  "home.runtime.probeFailed": "فشل فحص بيئة التشغيل: {error}",
  "home.runtime.cpuOnly": "المعالج فقط",

  // Editor status bar.
  "editor.status.project": "المشروع: {path}",
  "editor.status.noSelection": "لا يوجد تحديد",
  "editor.status.selected":
    "{count, plural, one {عنصر واحد محدّد} two {عنصران محدّدان} few {# عناصر محدّدة} many {# عنصرًا محدّدًا} other {# عنصر محدّد}}",

  // Language switcher.
  "lang.label": "اللغة",
  "lang.aria": "تغيير اللغة",
  "lang.changed": "تم تغيير اللغة إلى {language}",
};
