// Vendored from Vercel AI Elements (registry.ai-sdk.dev/code-block.json),
// adapted. The panel is dark-fixed (main.tsx pins `.dark` on documentElement),
// so only the dark (oneDark) variant is rendered — the upstream light/dark pair
// is collapsed to one. Used by AgentStream to render read_file/file_write tool
// payloads as real syntax-highlighted code instead of raw JSON. Zero CDN —
// Prism + the one-dark theme are bundled from the npm package.
"use client";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { CheckIcon, CopyIcon } from "lucide-react";
import {
  type ComponentProps,
  createContext,
  type HTMLAttributes,
  type ReactNode,
  useContext,
  useState,
} from "react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import oneDark from "react-syntax-highlighter/dist/esm/styles/prism/one-dark";

export type CodeBlockProps = HTMLAttributes<HTMLDivElement> & {
  code: string;
  language: string;
  showLineNumbers?: boolean;
  children?: ReactNode;
};

const CodeBlockContext = createContext<{ code: string }>({ code: "" });

export function CodeBlock({
  code,
  language,
  showLineNumbers = false,
  className,
  children,
  ...props
}: CodeBlockProps) {
  return (
    <CodeBlockContext.Provider value={{ code }}>
      <div
        className={cn(
          "relative w-full overflow-hidden rounded-md border text-foreground",
          className,
        )}
        {...props}
      >
        <div className="relative">
          <SyntaxHighlighter
            codeTagProps={{ className: "font-mono text-xs" }}
            customStyle={{
              margin: 0,
              padding: "0.75rem",
              fontSize: "0.75rem",
              borderRadius: 0,
            }}
            language={language}
            lineNumberStyle={{
              color: "rgba(255,255,255,0.28)",
              minWidth: "2.25rem",
              paddingRight: "0.75rem",
            }}
            showLineNumbers={showLineNumbers}
            style={oneDark}
            wrapLongLines={false}
          >
            {code}
          </SyntaxHighlighter>
          {children && (
            <div className="absolute top-1.5 right-1.5 flex items-center gap-2">
              {children}
            </div>
          )}
        </div>
      </div>
    </CodeBlockContext.Provider>
  );
}

export type CodeBlockCopyButtonProps = ComponentProps<typeof Button> & {
  onCopy?: () => void;
  onError?: (error: Error) => void;
  timeout?: number;
};

export function CodeBlockCopyButton({
  onCopy,
  onError,
  timeout = 2000,
  children,
  className,
  ...props
}: CodeBlockCopyButtonProps) {
  const [isCopied, setIsCopied] = useState(false);
  const { code } = useContext(CodeBlockContext);

  const copyToClipboard = async () => {
    if (typeof window === "undefined" || !navigator.clipboard?.writeText) {
      onError?.(new Error("Clipboard API not available"));
      return;
    }
    try {
      await navigator.clipboard.writeText(code);
      setIsCopied(true);
      onCopy?.();
      setTimeout(() => setIsCopied(false), timeout);
    } catch (error) {
      onError?.(error as Error);
    }
  };

  const Icon = isCopied ? CheckIcon : CopyIcon;

  return (
    <Button
      type="button"
      aria-label="코드 복사"
      className={cn("size-7 bg-background/40 hover:bg-background/70", className)}
      onClick={copyToClipboard}
      size="icon"
      variant="ghost"
      {...props}
    >
      {children ?? <Icon size={13} className="text-muted-foreground" />}
    </Button>
  );
}
