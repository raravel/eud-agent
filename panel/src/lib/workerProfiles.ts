import i18n from "@/lib/i18n";

export const WORKER_PROFILES = ["research", "map_research", "implementation"] as const;
export type WorkerProfile = (typeof WORKER_PROFILES)[number];

export const WORKER_PROFILE_LABELS: Readonly<Record<WorkerProfile, string>> = {
  get research() { return i18n.t("messages.workerProfile.research"); },
  get map_research() { return i18n.t("messages.workerProfile.map_research"); },
  get implementation() { return i18n.t("messages.workerProfile.implementation"); },
};
