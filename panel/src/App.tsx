import Editor from "@monaco-editor/react";
import {
  Conversation,
  ConversationContent,
} from "@/components/ai-elements/conversation";
import { Message, MessageContent } from "@/components/ai-elements/message";
// Side-effect import: binds Monaco to the local npm bundle (no CDN loader).
import "@/editor/monaco";

// Placeholder shell — build-level proof that AI Elements (Conversation /
// Message) and Monaco both wire up against the local bundle. The real panel
// (WS client, picker, review tabs, apply bar) lands in later tasks.
export default function App() {
  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center justify-between border-b px-4 py-2">
        <span className="font-semibold">EUD 에이전트</span>
        <span className="text-muted-foreground text-sm">패널 스캐폴드</span>
      </header>

      <Conversation>
        <ConversationContent>
          <Message from="assistant">
            <MessageContent>
              안녕하세요. 지시 사항을 입력하면 epScript 코드를 생성합니다.
            </MessageContent>
          </Message>
        </ConversationContent>
      </Conversation>

      <section className="border-t" aria-label="코드 편집기">
        <div className="px-4 py-2 text-muted-foreground text-sm">코드 편집</div>
        <Editor
          height="240px"
          defaultLanguage="plaintext"
          defaultValue={"// 적용할 epScript 코드가 여기에 표시됩니다\n"}
          options={{ minimap: { enabled: false }, fontSize: 13 }}
        />
      </section>
    </div>
  );
}
