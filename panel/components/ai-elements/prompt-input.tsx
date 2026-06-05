// Vendored from Vercel AI Elements (registry.ai-sdk.dev/prompt-input.json), decision 06.
//
// ADAPTATION (minimal subset): the upstream prompt-input.tsx is ~1000 lines and
// bundles file attachments (nanoid + `FileUIPart` from `ai`), speech recognition,
// a command palette (cmdk), model selects, dropdown action menus, hover cards and
// tabs — none of which the eud-agent panel uses (it sends a single `chat{text}`).
// Pulling all of that in would add several deps for zero benefit and bloat the
// no-runtime-CDN bundle. So only the chat-relevant primitives are vendored, each
// kept FAITHFUL to the upstream implementation:
//   PromptInput (form), PromptInputBody, PromptInputTextarea (Enter-to-submit with
//   IME-composition guard + disabled-submit guard), PromptInputFooter,
//   PromptInputTools, PromptInputButton, PromptInputSubmit.
// Dropped: attachments / speech / select / command / action-menu / hovercard /
//   tabs / the controller provider. (Reported in the task summary.)
"use client";

import { cn } from "@/lib/utils";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
} from "@/components/ui/input-group";
import { CornerDownLeftIcon, Loader2Icon, SquareIcon } from "lucide-react";
import {
  Children,
  type ComponentProps,
  type FormEvent,
  type FormEventHandler,
  type HTMLAttributes,
  type KeyboardEventHandler,
  useState,
} from "react";

/** Coarse submit status (subset of the upstream AI SDK ChatStatus). */
export type PromptInputStatus = "ready" | "submitted" | "streaming" | "error";

export type PromptInputProps = Omit<
  HTMLAttributes<HTMLFormElement>,
  "onSubmit"
> & {
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
};

export const PromptInput = ({
  className,
  onSubmit,
  children,
  ...props
}: PromptInputProps) => {
  const handleSubmit: FormEventHandler<HTMLFormElement> = (event) => {
    event.preventDefault();
    onSubmit(event);
  };

  return (
    <form
      className={cn("w-full", className)}
      onSubmit={handleSubmit}
      {...props}
    >
      <InputGroup>{children}</InputGroup>
    </form>
  );
};

export type PromptInputBodyProps = HTMLAttributes<HTMLDivElement>;

export const PromptInputBody = ({
  className,
  ...props
}: PromptInputBodyProps) => (
  <div className={cn("contents", className)} {...props} />
);

export type PromptInputTextareaProps = ComponentProps<typeof InputGroupTextarea>;

export const PromptInputTextarea = ({
  className,
  placeholder = "무엇을 만들까요?",
  ...props
}: PromptInputTextareaProps) => {
  const [isComposing, setIsComposing] = useState(false);

  const handleKeyDown: KeyboardEventHandler<HTMLTextAreaElement> = (e) => {
    if (e.key === "Enter") {
      if (isComposing || e.nativeEvent.isComposing) {
        return;
      }
      if (e.shiftKey) {
        return;
      }
      e.preventDefault();
      // Respect a disabled submit button (send gating v2).
      const form = e.currentTarget.form;
      const submitButton = form?.querySelector(
        'button[type="submit"]',
      ) as HTMLButtonElement | null;
      if (submitButton?.disabled) {
        return;
      }
      form?.requestSubmit();
    }
  };

  return (
    <InputGroupTextarea
      className={cn(className)}
      onCompositionEnd={() => setIsComposing(false)}
      onCompositionStart={() => setIsComposing(true)}
      onKeyDown={handleKeyDown}
      placeholder={placeholder}
      {...props}
    />
  );
};

export type PromptInputFooterProps = HTMLAttributes<HTMLDivElement>;

export const PromptInputFooter = ({
  className,
  ...props
}: PromptInputFooterProps) => (
  <InputGroupAddon
    align="block-end"
    className={cn("justify-between gap-1", className)}
    {...props}
  />
);

export type PromptInputToolsProps = HTMLAttributes<HTMLDivElement>;

export const PromptInputTools = ({
  className,
  ...props
}: PromptInputToolsProps) => (
  <div className={cn("flex items-center gap-1", className)} {...props} />
);

export type PromptInputButtonProps = ComponentProps<typeof InputGroupButton>;

export const PromptInputButton = ({
  variant = "ghost",
  className,
  size,
  ...props
}: PromptInputButtonProps) => {
  const newSize =
    size ?? (Children.count(props.children) > 1 ? "sm" : "icon-sm");

  return (
    <InputGroupButton
      className={cn(className)}
      size={newSize}
      type="button"
      variant={variant}
      {...props}
    />
  );
};

export type PromptInputSubmitProps = ComponentProps<typeof InputGroupButton> & {
  status?: PromptInputStatus;
};

export const PromptInputSubmit = ({
  className,
  variant = "default",
  size = "sm",
  status,
  children,
  ...props
}: PromptInputSubmitProps) => {
  let Icon = <CornerDownLeftIcon className="size-4" />;

  if (status === "submitted" || status === "streaming") {
    Icon =
      status === "streaming" ? (
        <SquareIcon className="size-4" />
      ) : (
        <Loader2Icon className="size-4 animate-spin" />
      );
  }

  return (
    <InputGroupButton
      className={cn(className)}
      size={size}
      type="submit"
      variant={variant}
      {...props}
    >
      {children ?? Icon}
    </InputGroupButton>
  );
};
