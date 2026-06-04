// Monaco wired to the LOCAL npm bundle — the @monaco-editor/react default CDN
// loader is FORBIDDEN (rules.md "Server and panel"). Every byte (incl. the
// editor workers) is bundled by Vite and served from the local origin.
//
// 1) Vite `?worker` imports compile each Monaco worker into a local bundle.
// 2) self.MonacoEnvironment.getWorker returns those local worker instances.
// 3) loader.config({ monaco }) points @monaco-editor/react at this bundle so
//    it never fetches monaco from a CDN.
import * as monaco from "monaco-editor";
import { loader } from "@monaco-editor/react";

import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import CssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import HtmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import TsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";

self.MonacoEnvironment = {
  getWorker(_workerId: string, label: string) {
    switch (label) {
      case "json":
        return new JsonWorker();
      case "css":
      case "scss":
      case "less":
        return new CssWorker();
      case "html":
      case "handlebars":
      case "razor":
        return new HtmlWorker();
      case "typescript":
      case "javascript":
        return new TsWorker();
      default:
        return new EditorWorker();
    }
  },
};

// Bind @monaco-editor/react to the local bundle (no CDN download).
loader.config({ monaco });

export { monaco };
