import type {
  ProviderAvailability,
  ProviderId,
  ProviderStatusCode,
  ReasoningLevel,
} from "./types";

export const PROVIDER_LABELS: Readonly<Record<ProviderId, string>> = {
  codex: "Codex",
  "claude-code": "Claude Code",
  antigravity: "Antigravity",
  "opencode-go": "OpenCode Go",
  ollama: "Ollama",
};

export const PROVIDER_DESCRIPTIONS: Readonly<Record<ProviderId, string>> = {
  codex: "공식 Codex CLI · ChatGPT 또는 API 키",
  "claude-code": "공식 Claude Code CLI · Pro/Max/Team/Enterprise 구독",
  antigravity: "Google OAuth · Cloud Code Assist",
  "opencode-go": "OpenCode Go API 키 · 모델별 공식 wire API",
  ollama: "로컬 또는 원격 Ollama · OpenAI 호환 API",
};

export const AVAILABILITY_LABELS: Readonly<
  Record<ProviderAvailability, string>
> = {
  ready: "연결됨",
  "needs-install": "설치 필요",
  "needs-authentication": "로그인 필요",
  "needs-credential": "API 키 필요",
  degraded: "확인 필요",
  unavailable: "사용 불가",
};

export const REASONING_LABELS: Readonly<Record<ReasoningLevel, string>> = {
  none: "추론 없음",
  minimal: "최소",
  low: "낮음",
  medium: "보통",
  high: "높음",
  xhigh: "매우 높음",
  max: "최대",
};

const ERROR_COPY: Readonly<Partial<Record<ProviderStatusCode, string>>> = {
  provider_not_installed: "공식 실행 파일을 설치해 주세요.",
  provider_not_authenticated: "이 앱 전용 프로필로 다시 로그인해 주세요.",
  provider_oauth_exchange_failed:
    "Google 로그인 결과를 확인하지 못했습니다. Google 로그인을 다시 시도해 주세요.",
  provider_oauth_client_unconfigured:
    "이 빌드에 Antigravity OAuth client가 설정되지 않았습니다. 배포 설정을 확인해 주세요.",
  provider_cloud_code_unauthorized:
    "Google 로그인은 완료됐지만 Cloud Code Assist가 요청을 거부했습니다. Google 계정을 확인한 뒤 다시 로그인해 주세요.",
  provider_account_ineligible:
    "이 Google 계정은 Antigravity를 사용할 수 없습니다. 계정 자격을 확인하거나 다른 Google 계정으로 로그인해 주세요.",
  provider_onboarding_required:
    "Cloud Code Assist 초기 설정을 완료하지 못했습니다. 잠시 후 다시 로그인해 주세요.",
  provider_auth_expired: "로그인이 만료되었습니다. 다시 연결해 주세요.",
  provider_credential_missing: "API 키 또는 OAuth 로그인이 필요합니다.",
  provider_credential_store_unavailable:
    "Windows 자격 증명 저장소에 로그인 정보를 저장하지 못했습니다. 앱을 다시 시작한 뒤 로그인해 주세요.",
  provider_import_unavailable:
    "가져올 수 있는 개인 CLI 자격 증명이 없습니다. 앱에서 로그인해 주세요.",
  provider_catalog_unavailable: "모델 목록을 불러오지 못했습니다. 다시 시도해 주세요.",
  provider_model_unavailable: "선택한 모델이 더 이상 제공되지 않습니다.",
  provider_capability_unsupported: "이 모델은 요청한 입력이나 추론 설정을 지원하지 않습니다.",
  provider_endpoint_invalid:
    "Base URL을 확인해 주세요. 로컬 HTTP는 localhost/loopback만, 원격 연결은 HTTPS만 지원합니다.",
  provider_rate_limited: "요청 한도에 도달했습니다. 잠시 후 같은 제공자로 다시 시도해 주세요.",
  provider_quota_exhausted: "제공자 사용량이 소진되었습니다. 다른 제공자로 자동 전환하지 않았습니다.",
  provider_protocol_changed: "제공자 응답 형식이 변경되어 안전하게 중단했습니다.",
  provider_transport_closed: "제공자 연결이 끊겼습니다. 요청은 다른 제공자로 전송되지 않았습니다.",
  provider_structured_output_invalid: "제공자가 요구된 구조의 결과를 반환하지 않았습니다.",
  provider_busy: "이 제공자를 사용하는 작업이 끝난 뒤 다시 시도해 주세요.",
  provider_cancelled: "연결 작업이 취소되었습니다.",
};
function normalizeProviderErrorCode(value: string): ProviderStatusCode | undefined {
  const stableCode = (value.match(/provider_[a-z_]+/g) ?? []).find((candidate) =>
    Object.prototype.hasOwnProperty.call(ERROR_COPY, candidate),
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
    return "Google 인증이 유효하지 않습니다. Google 로그인을 다시 시도해 주세요.";
  }
  if (normalized) return ERROR_COPY[normalized];
  if (provider === "antigravity") return ERROR_COPY.provider_protocol_changed;
  return "제공자 작업을 완료하지 못했습니다.";
}
