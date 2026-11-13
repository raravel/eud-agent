import { useTranslation } from "react-i18next";

/** Native project availability notice. */
export interface ConnectionNoticeProps {}

export function ConnectionNotice(_props: ConnectionNoticeProps) {
  const { t } = useTranslation();
  return (
    <section
      role="status"
      aria-label={t("app.connection.label")}
      className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm text-amber-200"
    >
      <span className="font-medium">{t("app.connection.title")}</span>{" "}
      <span className="text-amber-100/90">
        {t("app.connection.hint")}
      </span>
    </section>
  );
}
