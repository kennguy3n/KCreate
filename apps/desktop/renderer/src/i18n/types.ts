import { en } from "./locales/en";

/** Exhaustive union of every catalog key, derived from the English source. */
export type MessageKey = keyof typeof en;

/** A complete catalog (English). */
export type Messages = Record<MessageKey, string>;

/**
 * A translation catalog. Locales other than English are partial: any
 * omitted key falls back to the English value, so adding a key to
 * `en.ts` never forces a synchronous update to every locale.
 */
export type PartialMessages = Partial<Messages>;

/** Supported locale identifiers (BCP-47 tags). */
export type LocaleId = "en" | "es" | "ar";

/** Text direction for a locale. */
export type Direction = "ltr" | "rtl";

export interface LocaleMeta {
  readonly id: LocaleId;
  /** Endonym shown in the language switcher (always in its own script). */
  readonly nativeName: string;
  /** English name, used for screen-reader announcements. */
  readonly englishName: string;
  readonly dir: Direction;
}
