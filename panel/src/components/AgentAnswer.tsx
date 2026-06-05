/**
 * Live agent answer bubble (features/06 ## Behaviors → Answer prominence): the
 * streamed `delta` answer text renders into a PROMINENT (foreground) AI-Elements
 * Message/Response bubble via Streamdown, re-rendering as the text grows. Agent
 * answers are the most visible text in the log — foreground, in contrast to the
 * muted system/progress rows (this inverts the original v2 styling).
 *
 * Renders nothing while the answer buffer is empty (no bubble before the first
 * delta arrives).
 */
import { Message, MessageContent } from "@/components/ai-elements/message";
import { Response } from "@/components/ai-elements/response";

export interface AgentAnswerProps {
  /** The accumulated answer text (store turn.answer or a final answer{}). */
  text: string;
}

export function AgentAnswer({ text }: AgentAnswerProps) {
  if (text === "") return null;
  return (
    <Message from="assistant" className="text-foreground">
      <MessageContent>
        <Response>{text}</Response>
      </MessageContent>
    </Message>
  );
}
