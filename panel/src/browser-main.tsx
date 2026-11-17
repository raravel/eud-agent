import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import BrowserChrome from "@/browser/BrowserChrome";
import { initPanelLanguage } from "@/lib/i18n";

document.documentElement.classList.add("dark");
const root = document.getElementById("root");
if (!root) throw new Error("browser root element is missing");

await initPanelLanguage();

createRoot(root).render(
  <StrictMode>
    <BrowserChrome />
  </StrictMode>,
);
