import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "@/App";
import "@/index.css";

// Dark editor-like theme: the panel is hosted in the editor's WebView2 and is
// always dark (features/03 ## UI layout). The Tailwind theme tokens activate
// under the `.dark` class (see index.css `@custom-variant dark`).
document.documentElement.classList.add("dark");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
