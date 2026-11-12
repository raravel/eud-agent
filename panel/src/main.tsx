import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import "@/index.css";
import i18n, { initPanelLanguage } from "@/lib/i18n";

// Dark editor-like theme: the panel is hosted in the editor's WebView2 and is
// always dark (features/03 ## UI layout). The Tailwind theme tokens activate
// under the `.dark` class (see index.css `@custom-variant dark`).
document.documentElement.classList.add("dark");
const Surface = lazy(() => import("@/App"));

await initPanelLanguage();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Suspense fallback={<div className="flex h-dvh items-center justify-center bg-background text-sm text-muted-foreground">{i18n.t("common.loading")}</div>}>
      <Surface />
    </Suspense>
  </StrictMode>,
);
