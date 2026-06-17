// Public surface of the in-house i18n layer.
export { LocaleProvider, useI18n } from "./LocaleProvider";
export type { I18nContextValue } from "./LocaleProvider";
export { LOCALES, DEFAULT_LOCALE, localeMeta, asLocaleId } from "./catalog";
export { formatMessage } from "./format";
export type { MessageVars } from "./format";
export type {
  Direction,
  LocaleId,
  LocaleMeta,
  MessageKey,
  Messages,
} from "./types";
