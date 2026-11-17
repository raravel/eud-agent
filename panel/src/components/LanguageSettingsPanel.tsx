import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  loadUiLanguage,
  saveUiLanguage,
  type UiLanguage,
  type UiLanguageState,
} from "@/lib/i18n";

type Choice = "system" | UiLanguage;

const choiceOf = (state: UiLanguageState): Choice => (state.chosen ? state.language : "system");

/** Settings → 언어: follow the OS, or pick Korean or English. */
export function LanguageSettingsPanel({
  load = loadUiLanguage,
  save = saveUiLanguage,
}: {
  load?: () => Promise<UiLanguageState>;
  save?: (language: UiLanguage | null) => Promise<UiLanguageState>;
}) {
  const { t } = useTranslation();
  const headingId = useId();
  const [state, setState] = useState<UiLanguageState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    load()
      .then((loaded) => alive && setState(loaded))
      .catch((reason: unknown) => alive && setError(String(reason)));
    return () => {
      alive = false;
    };
  }, [load]);

  const choose = async (choice: Choice) => {
    setBusy(true);
    setError("");
    try {
      setState(await save(choice === "system" ? null : choice));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const options: Array<{ value: Choice; label: string; hint?: string }> = [
    {
      value: "system",
      label: t("settings.language.system"),
      hint:
        state && !state.chosen
          ? t("settings.language.systemCurrent", {
              language: t(`settings.language.${state.language}`),
            })
          : undefined,
    },
    { value: "ko", label: t("settings.language.ko") },
    { value: "en", label: t("settings.language.en") },
  ];

  return (
    <section
      aria-labelledby={headingId}
      className="animate-in fade-in duration-200 motion-reduce:animate-none"
    >
      <h2 id={headingId} className="text-base font-semibold">
        {t("settings.language.title")}
      </h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        {t("settings.language.description")}
      </p>
      <RadioGroup
        aria-label={t("settings.language.groupLabel")}
        className="mt-5 rounded-xl border border-border bg-card/40 p-4"
        value={state ? choiceOf(state) : ""}
        disabled={state === null || busy}
        onValueChange={(value) => void choose(value as Choice)}
      >
        {options.map((option) => {
          const id = `${headingId}-${option.value}`;
          return (
            <div key={option.value} className="flex min-h-11 items-center gap-3">
              <RadioGroupItem id={id} value={option.value} />
              <Label htmlFor={id} className="flex flex-col items-start gap-0.5 font-normal">
                <span>{option.label}</span>
                {option.hint && (
                  <span className="text-xs text-muted-foreground">{option.hint}</span>
                )}
              </Label>
            </div>
          );
        })}
      </RadioGroup>
      <p className="mt-3 text-xs text-muted-foreground">{t("settings.language.agentNote")}</p>
      {error && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {t("settings.language.saveFailed", { error })}
        </p>
      )}
    </section>
  );
}
