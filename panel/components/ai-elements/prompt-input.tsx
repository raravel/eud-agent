// Vendored from Vercel AI Elements (registry.ai-sdk.dev/prompt-input.json), decision 06.
//
// ADAPTATION (minimal subset): the upstream prompt-input.tsx is ~1000 lines and
// bundles its attachment state through nanoid + `FileUIPart` from `ai`, plus speech,
// cmdk, model selects, action menus, hover cards and tabs. EUD Agent keeps attachment
// bytes/session ownership in Rust and renders its lightweight tray in InstructionBox,
// so none of those upstream dependencies are needed. Only these primitives remain:
//   PromptInput (form), PromptInputBody, PromptInputTextarea (Enter-to-submit with
//   IME-composition guard + disabled-submit guard), PromptInputFooter,
//   PromptInputTools, PromptInputButton, PromptInputSubmit.
// Dropped upstream subsystems: attachment controller/provider (not attachment support),
// speech, select, command, action-menu, hovercard and tabs.
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
  onCompositionEnd,
  onCompositionStart,
  onKeyDown,
  ...props
}: PromptInputTextareaProps) => {
  const [isComposing, setIsComposing] = useState(false);

  const handleKeyDown: KeyboardEventHandler<HTMLTextAreaElement> = (e) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
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
      className={cn("min-h-16 max-h-48 shrink-0 field-sizing-content overflow-y-auto", className)}
      onCompositionEnd={(event) => {
        setIsComposing(false);
        onCompositionEnd?.(event);
      }}
      onCompositionStart={(event) => {
        setIsComposing(true);
        onCompositionStart?.(event);
      }}
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
    size ?? (Children.count(props.children) > 1 ? "default" : "icon");

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
  size = "default",
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
