import { ar } from "./locales/ar";
import { en } from "./locales/en";
import { es } from "./locales/es";
import type { LocaleId, LocaleMeta, Messages, PartialMessages } from "./types";

// Locale metadata, in switcher display order. English first (the
// source/default), then the shipped translations. `dir` drives the
// `document.documentElement.dir` flip performed by the provider.
export const LOCALES: readonly LocaleMeta[] = [
  { id: "en", nativeName: "English", englishName: "English", dir: "ltr" },
  { id: "es", nativeName: "Español", englishName: "Spanish", dir: "ltr" },
  { id: "ar", nativeName: "العربية", englishName: "Arabic", dir: "rtl" },
];

// Catalog registry. English is complete (`Messages`); the others are
// partial and fall back to English per-key at format time.
const CATALOGS: Record<LocaleId, Messages | PartialMessages> = {
  en,
  es,
  ar,
};

export const DEFAULT_LOCALE: LocaleId = "en";

const LOCALE_BY_ID = new Map<LocaleId, LocaleMeta>(
  LOCALES.map((meta) => [meta.id, meta]),
);

/** Look up a locale's metadata, falling back to the default locale. */
export function localeMeta(id: LocaleId): LocaleMeta {
  return LOCALE_BY_ID.get(id) ?? LOCALE_BY_ID.get(DEFAULT_LOCALE)!;
}

/** Narrow an arbitrary string to a supported `LocaleId`, else `null`. */
export function asLocaleId(value: string | null | undefined): LocaleId | null {
  return value != null && LOCALE_BY_ID.has(value as LocaleId)
    ? (value as LocaleId)
    : null;
}

/**
 * Resolve a message for `locale`, falling back to the English source
 * when the locale omits the key. Returns the key itself as a last
 * resort so a missing English key is visible rather than blank.
 */
export function resolveMessage(locale: LocaleId, key: keyof Messages): string {
  const catalog = CATALOGS[locale];
  return catalog[key] ?? en[key] ?? key;
}
