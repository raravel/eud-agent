import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import "@/index.css";

// Dark editor-like theme: the panel is hosted in the editor's WebView2 and is
// always dark (features/03 ## UI layout). The Tailwind theme tokens activate
// under the `.dark` class (see index.css `@custom-variant dark`).
document.documentElement.classList.add("dark");
const Surface = lazy(() => import("@/App"));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Suspense fallback={<div className="flex h-dvh items-center justify-center bg-background text-sm text-muted-foreground">화면 로딩…</div>}>
      <Surface />
    </Suspense>
  </StrictMode>,
);
