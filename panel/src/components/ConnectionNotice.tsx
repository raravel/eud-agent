/** Native project availability notice. */
export interface ConnectionNoticeProps {}

export function ConnectionNotice(_props: ConnectionNoticeProps) {
  return (
    <section
      role="status"
      aria-label="Native 프로젝트 상태"
      className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm text-amber-200"
    >
      <span className="font-medium">Native 프로젝트를 열 수 없습니다.</span>{" "}
      <span className="text-amber-100/90">
        project.json과 프로젝트 경로를 확인해 주세요.
      </span>
    </section>
  );
}
