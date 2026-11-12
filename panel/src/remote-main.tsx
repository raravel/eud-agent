import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import { initPanelLanguage } from "@/lib/i18n";
import RemoteApp from "@/remote/RemoteApp";
import { registerRemoteServiceWorker } from "@/remote/serviceWorker";

document.documentElement.classList.add("dark");
const root = document.getElementById("root");
if (!root) throw new Error("Remote root element is missing");

await initPanelLanguage();

createRoot(root).render(
  <StrictMode>
    <RemoteApp />
  </StrictMode>,
);

void registerRemoteServiceWorker();
