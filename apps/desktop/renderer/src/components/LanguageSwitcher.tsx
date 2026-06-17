import { colors, font, radius } from "../styles/tokens";
import { useI18n, asLocaleId } from "../i18n";
import { Icon } from "./Icon";

// Language switcher. A native `<select>` is used deliberately: it is
// keyboard-complete and screen-reader-labelled out of the box (arrow
// keys cycle options, the listbox semantics come for free), it is the
// lightest possible control, and it renders correctly under RTL. Each
// option is shown in its own script (the endonym) so a user who can't
// read the current UI language can still find their own.
export function LanguageSwitcher(): JSX.Element {
  const { locale, setLocale, locales, t } = useI18n();
  return (
    <label style={wrapperStyle}>
      <Icon name="globe" size={14} title={t("lang.label")} />
      <span style={visuallyHiddenInline}>{t("lang.label")}</span>
      <select
        value={locale}
        onChange={(e) => {
          const next = asLocaleId(e.target.value);
          if (next) setLocale(next);
        }}
        aria-label={t("lang.aria")}
        data-testid="kcreate-language-switcher"
        style={selectStyle}
      >
        {locales.map((meta) => (
          <option key={meta.id} value={meta.id}>
            {meta.nativeName}
          </option>
        ))}
      </select>
    </label>
  );
}

const wrapperStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  border: `1px solid ${colors.border}`,
  background: colors.bgSoft,
  color: colors.textMuted,
  borderRadius: radius.pill,
  paddingInline: 10,
  paddingBlock: 4,
};

const selectStyle: React.CSSProperties = {
  border: "none",
  background: "transparent",
  color: colors.text,
  fontFamily: font.family,
  fontSize: 12,
  fontWeight: 500,
  cursor: "pointer",
  appearance: "none",
};

// Inline visually-hidden label so the control keeps an accessible name
// even though only the globe glyph shows.
const visuallyHiddenInline: React.CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  overflow: "hidden",
  clip: "rect(0, 0, 0, 0)",
  whiteSpace: "nowrap",
};
