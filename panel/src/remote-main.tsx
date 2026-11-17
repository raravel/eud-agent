import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import i18n, { initPanelLanguage } from "@/lib/i18n";
import RemoteApp from "@/remote/RemoteApp";
import { registerRemoteServiceWorker } from "@/remote/serviceWorker";

document.documentElement.classList.add("dark");
const root = document.getElementById("root");
if (!root) throw new Error("Remote root element is missing");

await initPanelLanguage();
// The tab title the phone browser shows follows the page language too.
const syncTitle = () => {
  document.title = i18n.t("remote.app.title");
};
syncTitle();
i18n.on("languageChanged", syncTitle);

createRoot(root).render(
  <StrictMode>
    <RemoteApp />
  </StrictMode>,
);

void registerRemoteServiceWorker();
