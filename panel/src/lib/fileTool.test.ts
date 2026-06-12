import { describe, it, expect } from "vitest";
import { parseFileTool, decodeJsonField, languageForPath } from "./fileTool";

describe("decodeJsonField", () => {
  it("reads a field out of whole compact JSON", () => {
    expect(decodeJsonField('{"path":"a.eps","code":"x()"}', "code")).toBe("x()");
    expect(decodeJsonField('{"path":"a.eps","code":"x()"}', "path")).toBe("a.eps");
  });

  it("unescapes JSON string escapes", () => {
    expect(decodeJsonField('{"code":"a\\nb\\t\\"c\\""}', "code")).toBe('a\nb\t"c"');
  });

  it("recovers fields from TRUNCATED JSON (cut mid-string)", () => {
    const raw = '{"path":"a.eps","content":"line1\\nline2 …(잘림)';
    expect(decodeJsonField(raw, "path")).toBe("a.eps");
    expect(decodeJsonField(raw, "content")).toBe("line1\nline2");
  });

  it("returns null when the field is absent or the input is empty", () => {
    expect(decodeJsonField('{"ok":true}', "content")).toBeNull();
    expect(decodeJsonField(undefined, "code")).toBeNull();
    expect(decodeJsonField("not json at all", "code")).toBeNull();
  });
});

describe("languageForPath", () => {
  it("maps known extensions to Prism languages", () => {
    expect(languageForPath("scripts/a.py")).toBe("python");
    expect(languageForPath("triggers/main.eps")).toBe("javascript");
    expect(languageForPath("data.json")).toBe("json");
    expect(languageForPath("mod.lua")).toBe("lua");
  });

  it("defaults to plain text for unknown/extensionless paths", () => {
    expect(languageForPath("ui/layout.cui")).toBe("text");
    expect(languageForPath("README")).toBe("text");
  });
});

describe("parseFileTool", () => {
  it("parses file_write code from the ARGS", () => {
    const view = parseFileTool({
      id: "1",
      name: "file_write",
      state: "done",
      args: '{"path":"triggers/main.eps","code":"foo()"}',
      detail: '{"ok":true,"result":"saved"}',
    });
    expect(view).toEqual({
      mode: "write",
      path: "triggers/main.eps",
      code: "foo()",
      language: "javascript",
      truncated: false,
    });
  });

  it("parses read_file content from the RESULT", () => {
    const view = parseFileTool({
      id: "1",
      name: "read_file",
      state: "done",
      args: '{"path":"scripts/a.py"}',
      detail: '{"path":"scripts/a.py","content":"print(1)"}',
    });
    expect(view?.mode).toBe("read");
    expect(view?.code).toBe("print(1)");
    expect(view?.language).toBe("python");
  });

  it("flags truncation and still recovers the partial code", () => {
    const view = parseFileTool({
      id: "1",
      name: "file_write",
      state: "done",
      args: '{"path":"a.eps","code":"longcode …(잘림)',
    });
    expect(view?.truncated).toBe(true);
    expect(view?.code).toBe("longcode");
  });

  it("returns null for non-file tools (keeps the raw JSON view)", () => {
    expect(
      parseFileTool({ id: "1", name: "dat_set", state: "done", args: "{}" }),
    ).toBeNull();
  });

  it("returns null when read_file has no extractable content (error result)", () => {
    expect(
      parseFileTool({
        id: "1",
        name: "read_file",
        state: "failed",
        args: '{"path":"a.eps"}',
        detail: "ERROR: file not found",
      }),
    ).toBeNull();
  });
});
