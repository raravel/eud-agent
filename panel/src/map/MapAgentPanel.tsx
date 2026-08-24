import { History } from "lucide-react";

import { AskCard } from "@/components/AskCard";
import { ConversationLog } from "@/components/ConversationLog";
import { Button } from "@/components/ui/button";
import type {
  ChatAttachment,
  CodexModelSettings,
  ContextUsage,
} from "@/lib/ipc";
import type { AskAnswer, AskQuestion } from "@/lib/protocol";
import type { LogEntry, TurnState } from "@/state/store";
import type {
  MapMentionSnapshot,
  MentionChip,
  MentionQualifiers,
  SavedSelection,
} from "./mapProtocol";
import { MentionTray } from "./MentionTray";
import { QualifierEditor } from "./QualifierEditor";
import { MapPromptInput } from "./MapPromptInput";

export interface MapConversationEntry extends LogEntry {
  mapMentions?: MapMentionSnapshot[];
}

export interface MapAgentPanelProps {
  sessionName: string;
  conversation: MapConversationEntry[];
  turn: TurnState;
  live: boolean;
  actionBusy?: boolean;
  contextUsage?: ContextUsage | null;
  codexSettings?: CodexModelSettings | null;
  codexSettingsBusy?: boolean;
  mentions: MentionChip[];
  selectedMentionId?: string;
  ask?: { requestId: string; questions: AskQuestion[]; submitting: boolean };
  selections: SavedSelection[];
  mapWidth: number;
  mapHeight: number;
  draftScope: string;
  onSend(text: string, attachments: ChatAttachment[]): void;
  onCancel(): void;
  onStageAttachment?(file: File): Promise<ChatAttachment>;
  onDiscardAttachment?(id: string): Promise<void>;
  onCodexSettingsChange?(model: string, reasoningEffort: string): void;
  onCodexSettingsReload?(): void;
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
  codexSettings,
  codexSettingsBusy,
  mentions,
  selectedMentionId,
  ask,
  selections,
  mapWidth,
  mapHeight,
  draftScope,
  onSend,
  onCancel,
  onStageAttachment,
  onDiscardAttachment,
  onCodexSettingsChange,
  onCodexSettingsReload,
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
              onSubmit={(answers) => onAskSubmit(ask.requestId, answers)}
            />
          ) : undefined
        }
      />

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
        contextUsage={contextUsage}
        codexSettings={codexSettings}
        codexSettingsBusy={codexSettingsBusy}
        onSend={onSend}
        onCancel={onCancel}
        onStageAttachment={onStageAttachment}
        onDiscardAttachment={onDiscardAttachment}
        onCodexSettingsChange={onCodexSettingsChange}
        onCodexSettingsReload={onCodexSettingsReload}
      />
    </aside>
  );
}
