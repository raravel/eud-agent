export type ProviderId =
  | "codex"
  | "claude-code"
  | "antigravity"
  | "opencode-go"
  | "ollama";

export const PROVIDER_IDS: readonly ProviderId[] = [
  "codex",
  "claude-code",
  "antigravity",
  "opencode-go",
  "ollama",
] as const;

export type ProviderAvailability =
  | "ready"
  | "needs-install"
  | "needs-authentication"
  | "needs-credential"
  | "degraded"
  | "unavailable";

export type ProviderStatusCode =
  | "provider_not_installed"
  | "provider_not_authenticated"
  | "provider_oauth_exchange_failed"
  | "provider_oauth_client_unconfigured"
  | "provider_cloud_code_unauthorized"
  | "provider_account_ineligible"
  | "provider_onboarding_required"
  | "provider_auth_expired"
  | "provider_credential_missing"
  | "provider_credential_store_unavailable"
  | "provider_import_unavailable"
  | "provider_catalog_unavailable"
  | "provider_model_unavailable"
  | "provider_capability_unsupported"
  | "provider_rate_limited"
  | "provider_quota_exhausted"
  | "provider_endpoint_invalid"
  | "provider_protocol_changed"
  | "provider_transport_closed"
  | "provider_structured_output_invalid"
  | "provider_busy"
  | "provider_cancelled";

export type ReasoningLevel =
  | "none"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max";

export interface ReasoningSelection {
  level: string;
}

export interface ModelCapabilities {
  vision: boolean;
  toolCalls: boolean;
  strictStructuredOutput: boolean;
  reasoningLevels: ReasoningLevel[];
  nativeCompaction: boolean;
  contextWindow?: number;
  hostedWebSearch: boolean;
}

export interface ModelPrivacy {
  training: "not-used" | "used" | "unknown";
  retentionDays?: number;
  detail: string;
}

export function isProviderId(value: unknown): value is ProviderId {
  return PROVIDER_IDS.includes(value as ProviderId);
}

export function isProviderStatus(value: unknown): value is ProviderStatus {
  if (!value || typeof value !== "object") return false;
  const status = value as Record<string, unknown>;
  return (
    isProviderId(status.provider) &&
    [
      "ready",
      "needs-install",
      "needs-authentication",
      "needs-credential",
      "degraded",
      "unavailable",
    ].includes(status.availability as string) &&
    typeof status.selectedAsDefault === "boolean" &&
    typeof status.canInstall === "boolean" &&
    typeof status.canImport === "boolean" &&
    typeof status.experimental === "boolean" &&
    (status.detailCode === undefined ||
      status.detailCode === null ||
      typeof status.detailCode === "string")
  );
}
export interface ProviderModel {
  provider: ProviderId;
  model: string;
  displayName: string;
  description: string;
  isDefault: boolean;
  capabilities: ModelCapabilities;
  privacy?: ModelPrivacy;
}

export interface ProviderStatus {
  provider: ProviderId;
  availability: ProviderAvailability;
  selectedAsDefault: boolean;
  canInstall: boolean;
  canImport: boolean;
  experimental: boolean;
  detailCode?: ProviderStatusCode | null;
}

export interface ProviderSettingsView {
  provider: ProviderId;
  status: ProviderStatus;
  models: ProviderModel[];
  selectedModel?: string | null;
  selectedReasoning?: ReasoningSelection | null;
  version?: string | null;
  channel?: string | null;
  baseUrl?: string | null;
  hasApiKey: boolean;
}

export interface SessionModelSettings {
  provider: ProviderId;
  models: ProviderModel[];
  selectedModel: string;
  selectedReasoning?: ReasoningSelection;
}

export function isProviderModel(value: unknown): value is ProviderModel {
  if (!value || typeof value !== "object") return false;
  const model = value as Record<string, unknown>;
  const capabilities = model.capabilities as
    | Record<string, unknown>
    | undefined;
  return (
    isProviderId(model.provider) &&
    typeof model.model === "string" &&
    typeof model.displayName === "string" &&
    typeof model.description === "string" &&
    typeof model.isDefault === "boolean" &&
    !!capabilities &&
    typeof capabilities.vision === "boolean" &&
    typeof capabilities.toolCalls === "boolean" &&
    typeof capabilities.strictStructuredOutput === "boolean" &&
    Array.isArray(capabilities.reasoningLevels) &&
    typeof capabilities.nativeCompaction === "boolean" &&
    typeof capabilities.hostedWebSearch === "boolean"
  );
}
export type ProviderConversationState =
  | { provider: "codex"; threadId?: string }
  | { provider: "claude-code"; sessionId?: string }
  | { provider: "antigravity"; transcriptRevision: number }
  | { provider: "opencode-go"; transcriptRevision: number }
  | { provider: "ollama"; transcriptRevision: number };

export interface ProviderBinding {
  provider: ProviderId;
  model: string;
  reasoning?: ReasoningSelection;
  baseUrl?: string;
  conversation: ProviderConversationState;
}

export interface ProviderProgressEvent {
  provider: ProviderId;
  attemptId: string;
  stage: "install" | "login" | "catalog" | "refresh";
  percent?: number;
  detailCode?: string;
}
