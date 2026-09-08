import { describe, expect, it } from "vitest";
import {
  extractEudtoolsReferenceRows,
  extractEudtoolsWikiRows,
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

describe("selective eudtools extraction", () => {
  const snapshot = {
    root: "", slug: "Ar3sgice/eudtools.wiki",
    commit: "fba67326938424c005f6cbd94e8b9b385ad4e00c"
  };

  it("retains button constraints without GUI procedures or bypass advice", () => {
    const rows = extractEudtoolsWikiRows(`
### How to use Button Maker
Click Generate JSON.

### In addition to buttons
You also have to modify Dat Requirements for the buttons to work.

Always change to Always Use if you're not sure.

### Offset
It should be a pointer to empty memory.

When you use this tool more than once, keep the memory distinct.

### Sorting
It sorts buttons based on their positions.

Unsorted buttons may be displayed in the wrong place.

### Redirecting
It sets one unit's buttons to another unit's. Click Redirect.

### Changing unit command buttons dynamically
You can set buttons for unused units and use redirects.

Just remember that without the edited data redirects use the original buttons.
`, "Button-Maker", snapshot);
    const returned = rows.map((row) => row.content).join("\n");
    expect(returned).toContain("Dat Requirements");
    expect(returned).toContain("memory distinct");
    expect(returned).toContain("wrong place");
    expect(returned).toContain("original buttons");
    expect(returned).not.toMatch(/Always Use|Click|Generate JSON/);
    for (const row of rows) {
      expect(row.content).toContain("호환성 주의");
      expect(row.content).toContain("epScript가 아니다");
    }
  });

  it("keeps opcode operands and crash qualifiers, including malformed original spelling", () => {
    const rows = extractEudtoolsReferenceRows(
      "IceCC opcode name, opcode ID, parameters, opcode description\n\n" +
      "uflunstable 0x12 - <flingy# - creates a flingy; supposedly crashes in most cases.\n",
      "Data/iscriptopcodes.txt", { ...snapshot, slug: "Ar3sgice/eudtools" }
    );
    expect(rows[0].content).toContain(
      "uflunstable 0x12 - <flingy# - creates a flingy; supposedly crashes in most cases."
    );
  });

  it("does not separate animation continuation constraints from their slot", () => {
    const rows = extractEudtoolsReferenceRows(
      "+34 SpecialState1 - special animation,\n      used for special orders or construction\n" +
      "+36 SpecialState2 - burrowed animation",
      "Data/iscriptanimations.txt", { ...snapshot, slug: "Ar3sgice/eudtools" }
    );
    expect(rows[0].content).toContain(
      "+34 SpecialState1 - special animation,\n      used for special orders or construction"
    );
    expect(rows[1].content).toContain("+36 SpecialState2");
  });

  it("refuses documents outside the exact allowlist", () => {
    expect(() => extractEudtoolsWikiRows("unrelated", "Basic-Usage", snapshot)).toThrow(/allowlist/);
    expect(() => extractEudtoolsReferenceRows("unrelated", "Program/eudtools_data.js", snapshot)).toThrow(/allowlist/);
  });
});
