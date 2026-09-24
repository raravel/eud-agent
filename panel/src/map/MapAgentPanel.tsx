import { History, RotateCcw, TriangleAlert } from "lucide-react";

import { AskCard } from "@/components/AskCard";
import { ConversationLog } from "@/components/ConversationLog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type {
  ChatAttachment,
  ContextUsage,
  ReasoningSelection,
  SessionModelSettings,
} from "@/lib/ipc";
import type { AskAnswer, AskQuestion } from "@/lib/protocol";
import type { LogEntry, TurnState } from "@/state/store";
import type {
  MapLocation,
  MapMentionSnapshot,
  MentionChip,
  MentionQualifiers,
  SavedSelection,
} from "./mapProtocol";
import { MentionTray } from "./MentionTray";
import { QualifierEditor } from "./QualifierEditor";
import { MapPromptInput, type MapPromptDraft } from "./MapPromptInput";

export interface MapConversationEntry extends LogEntry {
  mapMentions?: MapMentionSnapshot[];
  /** The Map request a "you" entry started; lets a re-opened window skip a run it already shows. */
  requestId?: string;
}

export interface MapAgentPanelProps {
  sessionName: string;
  conversation: MapConversationEntry[];
  turn: TurnState;
  live: boolean;
  actionBusy?: boolean;
  contextUsage?: ContextUsage | null;
  modelSettings?: SessionModelSettings | null;
  modelSettingsBusy?: boolean;
  mentions: MentionChip[];
  selectedMentionId?: string;
  ask?: {
    requestId: string;
    questions: AskQuestion[];
    submitting: boolean;
    waitSeconds?: number;
    receivedAt?: number;
  };
  selections: SavedSelection[];
  /** Candidate locations offered by the prompt's `@` completion. */
  locations?: MapLocation[];
  mapWidth: number;
  mapHeight: number;
  draftScope: string;
  /** Why the saved provider conversation could not be resumed; chat is blocked until reset. */
  conversationResumeError?: string | null;
  conversationResetBusy?: boolean;
  onConversationReset?(): void;
  /** Rewind from a user message and restore it into the prompt for editing. */
  onEditMessage?(entry: MapConversationEntry): void;
  /** Disable the edit actions while a rewind is settling in the core. */
  editDisabled?: boolean;
  /** A past user message restored for editing after a successful rewind. */
  editDraft?: MapPromptDraft | null;
  onSend(text: string, attachments: ChatAttachment[]): void;
  onCancel(): void;
  onStageAttachment?(file: File): Promise<ChatAttachment>;
  onDiscardAttachment?(id: string): Promise<void>;
  onModelSettingsChange?(
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): void;
  onModelSettingsReload?(): void;
  onLocationMention?(location: MapLocation): void;
  onRegionMention?(selection: SavedSelection): void;
  onMentionSelect(id: string): void;
  onMentionRemove(id: string): void;
  onMentionFind(id: string): void;
  onMentionHighlight(id?: string): void;
  onQualifierChange(qualifiers: MentionQualifiers): void;
  onAskSubmit(requestId: string, answers: Record<string, AskAnswer>): void;
  onHistory(): void;
}

export function MapAgentPanel({
  sessionName,
  conversation,
  turn,
  live,
  actionBusy = false,
  contextUsage,
  modelSettings,
  modelSettingsBusy,
  mentions,
  selectedMentionId,
  ask,
  selections,
  locations = [],
  mapWidth,
  mapHeight,
  draftScope,
  conversationResumeError,
  conversationResetBusy = false,
  onConversationReset,
  onEditMessage,
  editDisabled = false,
  editDraft,
  onSend,
  onCancel,
  onStageAttachment,
  onDiscardAttachment,
  onModelSettingsChange,
  onModelSettingsReload,
  onLocationMention,
  onRegionMention,
  onMentionSelect,
  onMentionRemove,
  onMentionFind,
  onMentionHighlight,
  onQualifierChange,
  onAskSubmit,
  onHistory,
}: MapAgentPanelProps) {
  const selected = mentions.find((chip) => chip.id === selectedMentionId);
  return (
    <aside className="flex h-full min-h-0 min-w-0 flex-col border-l border-border bg-card/60">
      <div className="flex min-h-14 items-center gap-2 border-b border-border px-3 py-2">
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-sm font-semibold" title={sessionName}>
            {sessionName}
          </h2>
          <p className="truncate text-[11px] text-muted-foreground">
            Map Agent · 후보 draft만 수정
          </p>
        </div>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          className="size-11 shrink-0"
          aria-label="맵 작업 히스토리 열기"
          onClick={onHistory}
        >
          <History className="size-4" aria-hidden="true" />
        </Button>
      </div>
      <ConversationLog
        log={conversation}
        phase={live ? "thinking" : "ready"}
        turn={turn}
        emptyTitle="맵에 무엇을 만들까요?"
        emptyDescription="영역 권한과 팔레트 항목을 멘션에 담아 후보 맵을 만들어 보세요."
        onEditMessage={
          onEditMessage
            ? (entry) => onEditMessage(entry as MapConversationEntry)
            : undefined
        }
        editDisabled={editDisabled}
        renderUserMeta={(entry) => {
          const count =
            (entry as MapConversationEntry).mapMentions?.length ?? 0;
          return count > 0 ? (
            <span className="text-[11px] text-primary">
              구조화된 맵 멘션 {count}개
            </span>
          ) : null;
        }}
        tail={
          ask ? (
            <AskCard
              requestId={ask.requestId}
              questions={ask.questions}
              submitting={ask.submitting}
              waitSeconds={ask.waitSeconds}
              receivedAt={ask.receivedAt}
              onSubmit={(answers) => onAskSubmit(ask.requestId, answers)}
            />
          ) : undefined
        }
      />

      {conversationResumeError && (
        <Alert variant="destructive" className="mx-3 mb-2">
          <TriangleAlert aria-hidden="true" />
          <AlertTitle>이전 대화를 이어갈 수 없습니다</AlertTitle>
          <AlertDescription>
            <p>{conversationResumeError}</p>
            <p>대화를 초기화하면 새 모델 세션으로 이어서 요청할 수 있습니다. 맵 후보와 리비전, 대화 기록은 그대로 유지됩니다.</p>
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="min-h-11"
              disabled={conversationResetBusy || !onConversationReset}
              aria-busy={conversationResetBusy || undefined}
              onClick={() => onConversationReset?.()}
            >
              <RotateCcw className="size-4" aria-hidden="true" />
              {conversationResetBusy ? "초기화 중…" : "대화 초기화"}
            </Button>
          </AlertDescription>
        </Alert>
      )}

      <div className="max-h-[42%] space-y-2 overflow-y-auto border-t border-border p-3">
        <MentionTray
          chips={mentions}
          selectedId={selectedMentionId}
          onSelect={onMentionSelect}
          onRemove={onMentionRemove}
          onFind={onMentionFind}
          onHighlight={onMentionHighlight}
        />
        <QualifierEditor
          chip={selected}
          onChange={onQualifierChange}
          selections={selections}
          mapWidth={mapWidth}
          mapHeight={mapHeight}
        />
      </div>

      <MapPromptInput
        turn={turn}
        live={live}
        actionBusy={actionBusy}
        mentionCount={mentions.length}
        hasStaleMentions={mentions.some((chip) => chip.stale)}
        draftScope={draftScope}
        draft={editDraft}
        contextUsage={contextUsage}
        modelSettings={modelSettings}
        modelSettingsBusy={modelSettingsBusy}
        locations={locations}
        selections={selections}
        onLocationMention={onLocationMention}
        onRegionMention={onRegionMention}
        onSend={onSend}
        onCancel={onCancel}
        onStageAttachment={onStageAttachment}
        onDiscardAttachment={onDiscardAttachment}
        onModelSettingsChange={onModelSettingsChange}
        onModelSettingsReload={onModelSettingsReload}
      />
    </aside>
  );
}
