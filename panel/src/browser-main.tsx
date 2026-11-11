import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import BrowserChrome from "@/browser/BrowserChrome";

document.documentElement.classList.add("dark");
const root = document.getElementById("root");
if (!root) throw new Error("browser root element is missing");

createRoot(root).render(
  <StrictMode>
    <BrowserChrome />
  </StrictMode>,
);
