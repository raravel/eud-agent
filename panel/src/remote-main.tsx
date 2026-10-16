import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import RemoteApp from "@/remote/RemoteApp";

document.documentElement.classList.add("dark");
const root = document.getElementById("root");
if (!root) throw new Error("Remote root element is missing");

createRoot(root).render(
  <StrictMode>
    <RemoteApp />
  </StrictMode>,
);
