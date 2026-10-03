export const WORKER_PROFILES = ["research", "map_research", "implementation"] as const;
export type WorkerProfile = (typeof WORKER_PROFILES)[number];

export const WORKER_PROFILE_LABELS: Readonly<Record<WorkerProfile, string>> = {
  research: "코드·문서 조사",
  map_research: "맵 조사",
  implementation: "구현",
};
