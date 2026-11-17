import i18n from "@/lib/i18n";
import { isMacOS } from "@/lib/platform";

import type {
  ProviderAvailability,
  ProviderId,
  ProviderStatusCode,
  ReasoningLevel,
} from "./types";

/** A label table whose values are translated when read, so a language switch applies. */
function lazyLabels<K extends string>(
  keys: readonly K[],
  text: (key: K) => string,
): Readonly<Record<K, string>> {
  const table = {} as Record<K, string>;
  for (const key of keys) {
    Object.defineProperty(table, key, { enumerable: true, get: () => text(key) });
  }
  return table;
}

const PROVIDER_IDS: readonly ProviderId[] = [
  "codex",
  "claude-code",
  "antigravity",
  "opencode-go",
  "opencode-zen",
  "ollama",
];

export const PROVIDER_LABELS: Readonly<Record<ProviderId, string>> = lazyLabels(
  PROVIDER_IDS,
  (id) => i18n.t(`providers.label.${id}`),
);

export const PROVIDER_DESCRIPTIONS: Readonly<Record<ProviderId, string>> = lazyLabels(
  PROVIDER_IDS,
  (id) => i18n.t(`providers.description.${id}`),
);

export const AVAILABILITY_LABELS: Readonly<
  Record<ProviderAvailability, string>
> = lazyLabels(
  ["ready", "needs-install", "needs-authentication", "needs-credential", "degraded", "unavailable"],
  (key) => i18n.t(`providers.availability.${key}`),
);

export const REASONING_LABELS: Readonly<Record<ReasoningLevel, string>> = lazyLabels(
  ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
  (key) => i18n.t(`providers.reasoning.${key}`),
);

const ERROR_CODES: readonly ProviderStatusCode[] = [
  "provider_not_installed",
  "provider_not_authenticated",
  "provider_oauth_exchange_failed",
  "provider_oauth_client_unconfigured",
  "provider_cloud_code_unauthorized",
  "provider_account_ineligible",
  "provider_onboarding_required",
  "provider_auth_expired",
  "provider_auth_refresh_failed",
  "provider_login_failed",
  "provider_credential_missing",
  "provider_credential_store_unavailable",
  "provider_catalog_unavailable",
  "provider_model_unavailable",
  "provider_capability_unsupported",
  "provider_endpoint_invalid",
  "provider_rate_limited",
  "provider_quota_exhausted",
  "provider_protocol_changed",
  "provider_transport_closed",
  "provider_structured_output_invalid",
  "provider_busy",
  "provider_cancelled",
];

function errorCopyFor(code: ProviderStatusCode): string {
  if (code === "provider_credential_store_unavailable") {
    return i18n.t("providers.error.provider_credential_store_unavailable", {
      store: i18n.t(isMacOS() ? "providers.credentialStore.macos" : "providers.credentialStore.windows"),
    });
  }
  return i18n.t(`providers.error.${code}`);
}

function normalizeProviderErrorCode(value: string): ProviderStatusCode | undefined {
  const stableCode = (value.match(/provider_[a-z_]+/g) ?? []).find((candidate) =>
    (ERROR_CODES as readonly string[]).includes(candidate),
  );
  if (stableCode) return stableCode as ProviderStatusCode;
  if (
    value.includes("invalid provider status response") ||
    value.includes("invalid provider settings response") ||
    value.includes("invalid provider catalog response") ||
    value.includes("invalid provider login attempt")
  ) {
    return "provider_protocol_changed";
  }
  return undefined;
}


/**
 * A chat failure carries the runtime's raw error (`provider transport failed: <code>`).
 * Lead it with the code's recovery copy and keep the raw text for diagnosis.
 */
export function providerChatErrorText(message: string): string {
  const normalized = normalizeProviderErrorCode(message);
  if (!normalized || !message.includes(normalized)) return message;
  return `${errorCopyFor(normalized)} (${message})`;
}

export function providerErrorCopy(
  code: string | null | undefined,
  provider?: ProviderId,
): string | undefined {
  if (!code) return undefined;
  const normalized = normalizeProviderErrorCode(code);
  if (
    normalized === "provider_not_authenticated" &&
    provider === "antigravity"
  ) {
    return i18n.t("providers.error.antigravityNotAuthenticated");
  }
  if (normalized) return errorCopyFor(normalized);
  if (provider === "antigravity") return errorCopyFor("provider_protocol_changed");
  return i18n.t("providers.error.generic");
}
