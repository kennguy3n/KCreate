import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";

import { visuallyHidden } from "../a11y/visuallyHidden";
import {
  DEFAULT_LOCALE,
  LOCALES,
  asLocaleId,
  localeMeta,
  resolveMessage,
} from "./catalog";
import { formatMessage } from "./format";
import type { MessageVars } from "./format";
import type { Direction, LocaleId, LocaleMeta, MessageKey } from "./types";

const STORAGE_KEY = "kcreate.locale";

export interface I18nContextValue {
  readonly locale: LocaleId;
  readonly dir: Direction;
  readonly locales: readonly LocaleMeta[];
  /** Translate `key`, interpolating `vars` with the ICU-lite formatter. */
  readonly t: (key: MessageKey, vars?: MessageVars) => string;
  readonly setLocale: (locale: LocaleId) => void;
  /** Locale-aware number formatting via the platform `Intl`. */
  readonly formatNumber: (value: number) => string;
}

/**
 * Build the context value for a given locale. Pulled out so the
 * no-provider default (English) and the live provider share one code
 * path — `t`/`formatNumber` behave identically with or without a
 * mounted `LocaleProvider`, which keeps component tests that render a
 * surface bare (no provider) on the English catalog instead of
 * throwing.
 */
function makeValue(
  locale: LocaleId,
  setLocale: (locale: LocaleId) => void,
): I18nContextValue {
  const meta = localeMeta(locale);
  return {
    locale,
    dir: meta.dir,
    locales: LOCALES,
    t: (key, vars) => formatMessage(resolveMessage(locale, key), vars, locale),
    setLocale,
    formatNumber: (value) => new Intl.NumberFormat(locale).format(value),
  };
}

const noopSetLocale = (): void => {};

// Default value used when `useI18n` is called outside a provider:
// English, LTR, and a no-op setter. This is deliberate — many unit
// tests render a single component without wrapping it, and they assert
// on the English strings.
const DEFAULT_CONTEXT = makeValue(DEFAULT_LOCALE, noopSetLocale);

const I18nContext = createContext<I18nContextValue>(DEFAULT_CONTEXT);

/** Read the persisted locale, guarding for non-browser/test environments. */
function readStoredLocale(): LocaleId {
  if (typeof window === "undefined") return DEFAULT_LOCALE;
  try {
    return asLocaleId(window.localStorage.getItem(STORAGE_KEY)) ?? DEFAULT_LOCALE;
  } catch {
    // localStorage can throw (privacy mode, disabled storage) — fall
    // back to the default rather than crashing the whole renderer.
    return DEFAULT_LOCALE;
  }
}

export function LocaleProvider({
  children,
  initialLocale,
}: {
  children: ReactNode;
  /** Override the initial locale (used by tests). */
  initialLocale?: LocaleId;
}): JSX.Element {
  const [locale, setLocaleState] = useState<LocaleId>(
    () => initialLocale ?? readStoredLocale(),
  );
  // Last locale-change announcement, mirrored into a polite live region
  // so screen-reader users hear the switch confirmed in the new locale.
  const [announcement, setAnnouncement] = useState("");

  // The updater stays pure (just the next locale); every side effect
  // lives in the effect below. React can call a state updater more than
  // once (StrictMode, concurrent rendering), so persisting / announcing
  // inside it would be unsafe.
  const setLocale = useCallback((next: LocaleId) => {
    setLocaleState(next);
  }, []);

  // Locale as of the last committed effect run, so we can tell a real
  // switch apart from the initial mount (and StrictMode's double-invoke)
  // and only persist / announce when the locale actually changes.
  const prevLocaleRef = useRef(locale);

  // Reflect the active locale onto the document element so global CSS,
  // logical properties, and assistive tech all see the right language
  // and writing direction (the single place the app flips RTL); then,
  // on an actual change, persist the choice and announce it in a polite
  // live region so screen-reader users hear the switch confirmed in the
  // new locale.
  useEffect(() => {
    const meta = localeMeta(locale);
    if (typeof document !== "undefined") {
      document.documentElement.lang = locale;
      document.documentElement.dir = meta.dir;
    }
    if (prevLocaleRef.current === locale) return;
    prevLocaleRef.current = locale;
    try {
      window.localStorage.setItem(STORAGE_KEY, locale);
    } catch {
      // Persistence is best-effort; ignore storage failures.
    }
    setAnnouncement(
      formatMessage(
        resolveMessage(locale, "lang.changed"),
        { language: meta.nativeName },
        locale,
      ),
    );
  }, [locale]);

  const value = useMemo(() => makeValue(locale, setLocale), [locale, setLocale]);

  return (
    <I18nContext.Provider value={value}>
      {children}
      <div
        aria-live="polite"
        aria-atomic="true"
        data-testid="kcreate-locale-announcer"
        style={visuallyHidden}
      >
        {announcement}
      </div>
    </I18nContext.Provider>
  );
}

/** Access the active locale, the translator `t`, and locale controls. */
export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}
