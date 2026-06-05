// Vendored from Vercel AI Elements (registry.ai-sdk.dev/message.json), decision 06.
//
// ADAPTATION: the upstream message.tsx imports `UIMessage`/`FileUIPart` from the
// `ai` package (AI SDK) only to type the `from` role and the attachment/branch
// sub-components. This project does not use the AI SDK message shape, so:
//   - `from` is typed with a local `MessageRole` union (no `ai` dep);
//   - only `Message` + `MessageContent` are vendored (the branch-navigation,
//     attachment, and action sub-components are dropped — unused by the panel).
// The styling classes are kept verbatim so the prominent (foreground) agent
// bubble + the user-secondary bubble match upstream.
"use client";

import { cn } from "@/lib/utils";
import type { HTMLAttributes } from "react";

/** Local role union — replaces the AI SDK `UIMessage["role"]`. */
export type MessageRole = "user" | "assistant" | "system";

export type MessageProps = HTMLAttributes<HTMLDivElement> & {
  from: MessageRole;
};

export const Message = ({ className, from, ...props }: MessageProps) => (
  <div
    className={cn(
      "group flex w-full max-w-[95%] flex-col gap-2",
      from === "user" ? "is-user ml-auto justify-end" : "is-assistant",
      className,
    )}
    {...props}
  />
);

export type MessageContentProps = HTMLAttributes<HTMLDivElement>;

export const MessageContent = ({
  children,
  className,
  ...props
}: MessageContentProps) => (
  <div
    className={cn(
      "is-user:dark flex w-fit max-w-full min-w-0 flex-col gap-2 overflow-hidden text-sm",
      "group-[.is-user]:ml-auto group-[.is-user]:rounded-lg group-[.is-user]:bg-secondary group-[.is-user]:px-4 group-[.is-user]:py-3 group-[.is-user]:text-foreground",
      "group-[.is-assistant]:text-foreground",
      className,
    )}
    {...props}
  >
    {children}
  </div>
);
