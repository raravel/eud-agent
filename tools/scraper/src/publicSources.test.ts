import { describe, expect, it } from "vitest";
import {
  extractPythonDefinitions,
  extractPythonExports,
  parseDatDefinition,
  parseEditorFunctions,
  splitMarkdownSections
} from "./publicSources.js";

describe("splitMarkdownSections", () => {
  it("splits normal and list-indented headings while preserving fenced code", () => {
    const sections = splitMarkdownSections(`
# Built-in Functions

Intro.

- ### Conditions

  - #### **Bring**

    \`Bring(player, comparison, amount, unit, location)\`

    \`\`\`JavaScript
    // # this is code, not a heading
    if (Bring(P1, AtLeast, 1, $U("Terran Marine"), $L("Anywhere"))) {}
    \`\`\`
`);

    expect(sections.map((section) => section.title)).toEqual([
      "Built-in Functions",
      "Built-in Functions > Conditions",
      "Built-in Functions > Conditions > Bring"
    ]);
    expect(sections[2].content).toContain("# this is code, not a heading");
  });
});

describe("Python API extraction", () => {
  it("reads explicit exports and public definitions with class methods", () => {
    const source = `
__all__ = ["EUDVariable", "f_div"]

class EUDVariable:
    """Runtime variable."""

    def SetNumber(self, value):
        """Assign a value."""
        pass

    def _private(self):
        pass


def f_div(a, b):
    """Divide two values."""
    return a // b
`;

    expect(extractPythonExports(source)).toEqual(["EUDVariable", "f_div"]);
    expect(extractPythonDefinitions(source)).toEqual([
      {
        name: "EUDVariable",
        kind: "class",
        signature: "class EUDVariable:",
        documentation: "Runtime variable.",
        methods: ["def SetNumber(self, value):"]
      },
      {
        name: "f_div",
        kind: "function",
        signature: "def f_div(a, b):",
        documentation: "Divide two values.",
        methods: []
      }
    ]);
  });
});

describe("parseDatDefinition", () => {
  it("groups indexed DAT fields into exact parameter records", () => {
    expect(
      parseDatDefinition(`
[HEADER]
Varcount=2

[FORMAT]
0Name=Graphics
0Size=1
0Type=2

1Name=Hit Points
1Size=4
`)
    ).toEqual([
      { index: "0", Name: "Graphics", Size: "1", Type: "2" },
      { index: "1", Name: "Hit Points", Size: "4" }
    ]);
  });
});

describe("parseEditorFunctions", () => {
  it("keeps the editor-provided signature and bilingual documentation together", () => {
    expect(
      parseEditorFunctions(`
/***
 * @Summary.ko-KR
 * 유닛을 생성합니다.
 * @Summary.en-US
 * Create units.
***/
function CreateUnit(Count, Unit : TrgUnit, Where : TrgLocation, Player : TrgPlayer){}
`)
    ).toEqual([
      {
        name: "CreateUnit",
        documentation:
          "@Summary.ko-KR\n유닛을 생성합니다.\n@Summary.en-US\nCreate units.",
        signature:
          "function CreateUnit(Count, Unit : TrgUnit, Where : TrgLocation, Player : TrgPlayer){}"
      }
    ]);
  });
});
