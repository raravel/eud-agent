/**
 * Editor connection notice (EUD-120).
 *
 * Presentational banner for a stale/absent EUD Editor bridge heartbeat. App
 * renders it only while the store says the editor is disconnected.
 */
export interface ConnectionNoticeProps {}

export function ConnectionNotice(_props: ConnectionNoticeProps) {
  return (
    <section
      role="status"
      aria-label="에디터 연결 상태"
      className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm text-amber-200"
    >
      <span className="font-medium">에디터가 연결되지 않았습니다.</span>{" "}
      <span className="text-amber-100/90">
        EUD Editor 3을 실행하면 지시·적용이 다시 활성화됩니다.
      </span>
    </section>
  );
}
