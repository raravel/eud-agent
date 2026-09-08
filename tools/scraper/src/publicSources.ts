import { execFile } from "node:child_process";
import { mkdtemp, readdir, readFile, rename, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, sep } from "node:path";
import { corpusOutputDir } from "./config.js";
import {
  type CorpusJsonRow,
  writeCorpusJsonlAtomic
} from "./corpusWriter.js";

const SCR_SOURCE = "scrmapdocs_en.jsonl";
const EUDPLIB_API_SOURCE = "eudplib_api.jsonl";
const EUDPLIB_EXAMPLE_SOURCE = "eudplib_examples.jsonl";
const EDITOR_SOURCE = "eud_editor_schema.jsonl";
const EUD_BOOK_SOURCE = "eud_book.jsonl";

const EUDTOOLS_WIKI_SOURCE = "eudtools_wiki.jsonl";
const EUDTOOLS_REFERENCE_SOURCE = "eudtools_reference.jsonl";
const EUDTOOLS_EXPERIMENTAL_SOURCE = "eudtools_wiki_experimental.jsonl";
const EUDTOOLS_COMMIT = "e9729dc12cc30e575a83940ef380570d4819b5b2";
const EUDTOOLS_WIKI_COMMIT = "fba67326938424c005f6cbd94e8b9b385ad4e00c";
const EUDTOOLS_PERMISSION =
  "User confirmed on 2026-09-08 that these materials may be freely used. " +
  "Permission basis: user confirmation, not an upstream license grant. No license is assigned.";
const EUDTOOLS_CAUTION =
  "호환성 주의: 2020년 레거시 StarCraft/Brood War EUD 자료의 주장이다. " +
  "현 프로젝트에서 게임 동작을 검증하지 않았다. SCMDraft/Pure EUD 및 IceCC 코드는 epScript가 아니다.";

type WikiSelection = {
  heading: string;
  korean: string;
  // Optional exact paragraph prefixes restrict a section without importing GUI advice.
  paragraphs?: readonly string[];
  quotes?: readonly string[];
};

const eudtoolsWikiSelections: Record<string, readonly WikiSelection[]> = {
  "EUD-Tutorial:-Creating-uncreatable-units": [
    { heading: "Uncreatable units", korean: "생성 불가 유닛: 에디터에 보여도 트리거 생성은 별개다." },
    { heading: "How to solve the problem", korean: "생성 가능 플래그 0x02와 예외: Scarabs/Interceptors는 여전히 불가, 자원은 player 12 소유." },
    { heading: "Creating buildings off terrain boundaries", korean: "건물 배치 크기 Building Dimensions 31x31과 지형·유닛 점유 제약 우회에 관한 원문 예제." },
    { heading: "Creating units with a different color", korean: "소유자와 색상: 다른 플레이어로 생성 후 CUnit Player ID 변경. 예제의 고정 CUnit 주소를 재사용하지 말 것." }
  ],
  "EUD-Tutorial:-How-to-make-units-other-than-spellcasters-cast-spells": [
    { heading: "The Problem", korean: "일반 유닛에 마법 버튼과 요구사항을 추가해도 시전되지 않는 이유: CastSpell 애니메이션 슬롯 +7에 opcode 0x27 castspell이 필요하다." },
    { heading: "Spells that do not require this animation", korean: "CastSpell 없이 시전하는 예외: Stim Packs, Spider Mines, Scanner Sweep, Defensive Matrix, Recall. 원저자는 전체 마법을 시험하지 않았다고 명시한다." },
    { heading: "Iscript Index", korean: "iscript 교체는 images.dat Iscript Index로 기존 스크립트를 선택하는 것. 유닛 모델을 유지하지만 누락된 CastSpell과 프레임 호환성은 별도 확인해야 한다." },
    { heading: "Picking Iscripts", korean: "iscript 교체와 공격·프레임 호환성: Defiler를 다른 유닛이 사용하면 크래시가 난다는 원문 보고다. Ghost/Kerrigan 계열은 frameset 13–15까지 사용하여 이동 중 이미지가 주기적으로 제거될 수 있다. 원문은 CUnit 삭제가 아니라 이미지 소실을 설명한다. Medic과 Science Vessel은 공격하지 못한다. High/Dark Templar는 지상만 공격한다. 다른 후보도 완전 호환은 아니며, 추가 오버레이의 Drawing Function을 바꾸면 원래 유닛에도 영향을 준다." },
    { heading: "Using the Attack Animation with CUnit/Orders", korean: "시전 명령에 공격 애니메이션을 사용하면 마법 대신 무기를 쓴다. spellCooldown 감지로 무기 교체·복원을 조율해야 한다. 지면 타게팅 제한과 Use Weapon Targeting을 함께 검토한다." }
  ],
  "EUD-Tutorial:-Creating-Triggered-Spells": [
    { heading: "Locating CUnit address of your unit", korean: "트리거 스킬 감지는 시전자 CUnit 식별이 선행 조건. 배치 유닛의 고정 주소와 생성 유닛의 동적 포인터를 구분한다." },
    { heading: "Static CUnit for preplaced units", korean: "SCMDraft 0.9.10 배치 Map Index → CUnit Index 공식. 버전·배치 상태가 다른 맵에 고정 주소를 이식하지 않는다." },
    { heading: "Dynamic CUnit for units created with triggers", korean: "생성 직전에 next unit pointer를 저장하여 시전자 CUnit을 식별한다. 이미지의 EE3 절차를 현재 저작 API로 변환하지 않는다." },
    { heading: "Non-targeting spells", korean: "대기열로 감지하는 비대상 스킬: 비건물 유닛은 생산 진행도가 0%에서 멈추므로 이를 스킬을 발동하는 신호로 삼는다. 감지한 후에는 Build Queue 1–5를 모두 228 (None)로 초기화해야 한다. 0은 Marine이지 빈 슬롯이 아니다. Secondary Order도 2 (Idle)로 초기화해야 생산 시도가 끝난다. 큐만 비우면 빈 유닛을 만들려 하여 버그 스프라이트가 생길 수 있다." },
    { heading: "Targeting spells", korean: "목표 지정 스킬: Create Building의 Order Coordinates와 Main Order를 감지한다. Build (Protoss) 요구사항의 Probe 제한을 해제해야 한다. Secondary Order만 감지하면 이동 전에 처리하지 못한다." },
    { heading: "Targeting spells costing energy instead of resources", korean: "마나 소모 스킬: 주문 Animation을 ReturnToIdle, 무기 cooldown을 100으로 설정한다는 원문 방식. Spell Cooldown 감지 후 0으로 초기화하고 Order Coordinates를 읽는다. 원문 예제 감지 임계값은 80이며 완전한 현대 구현 예제가 아니다." }
  ],
  "EUD-Tutorial:-Extended-Animations": [
    { heading: "Orders.dat Animation", korean: "명령 Animation은 일부 마법 시전 명령에만 적용. 일반 Cast Spell 값은 7이고 예외 주문이 있다." },
    { heading: "Extended Animations", korean: "실험적 확장 애니메이션: 정상 0–27 밖의 값을 255까지 사용해 헤더 밖을 읽는 원문 연구. 버전·메모리 배치 의존이며 안전한 기능으로 취급하지 않는다.",
      paragraphs: ["In fact,", "When the value", "![]"] },
    { heading: "But if the animation has no castspell command the unit won't cast the spell! What's the point of doing this!?", korean: "실험적 명령 애니메이션: castspell이 없어도 마나 감소와 spell cooldown을 CUnit에서 감지한다는 원문 주장. 우연히 0x27에 도달하는 동작에 의존하지 않는다." },
    { heading: "Examples", korean: "실험 예: Stasis Field Animation 74로 Arbiter에 Archon being/swirl 추가. 공격 중복과 반복 시 오버레이 누적은 원문 주장이지 현재 게임 검증 결과가 아니다." },
    { heading: "Set Doodad State", korean: "실험적 SetDoodadState: Disable/Enable 슬롯이 짧은 가변 길이 iscript 헤더 밖으로 넘어갈 수 있다. Marine Disable이 0x0E80 Ensnare 코드로 해석되는 예는 버전 의존이다." }
  ],
  "EUD-Tutorial:-The-Rock-Sprite,-and-removing-unwanted-sprites-&-images": [
    { heading: "Overview", korean: "Rock Sprite는 비장식 유닛 SetDoodadState에서 드러나는 iscript 오류에 관한 설명." },
    { heading: "How to solve the rock sprite problem", korean: "리마스터에서 iscript.bin 직접 수정과 기존 iscript 선택은 다르다. 원문은 EUD로 iscript 본문을 편집할 수 없어 images.dat의 기존 Iscript Index를 바꾸는 대안을 설명한다. Image 589 → 276 (ShieldsOverlay). SC:R의 없는 프레임 제거와 미리 배치된 doodad 예외에 의존한다." },
    { heading: "How to remove other sprites and images", korean: "불필요한 스프라이트·이미지 제거: sprite의 Image Index 589는 앞 절의 Image 589 iscript 276 설정이 선행 조건. image overlay는 iscript 386, frames 204–255를 사용하지만 반복하므로 sprite에는 부적합하다." },
    { heading: "Why the Rock Sprite happens", korean: "iscript offset 0을 없는 애니메이션으로 처리하지 않아 sprol 0 0 132가 바위를 132 pixels 위에 생성한다는 분석. 리마스터 이전 engset 크래시와 Remastered 이미지 제거의 차이를 원문이 보고한다." }
  ],
  "Button-Maker": [
    { heading: "In addition to buttons", korean: "버튼과 Dat Requirements는 별개다. 버튼 리다이렉트만으로 시전·생산 요구사항을 충족하지 않는다. 모르면 요구사항을 무조건 통과시키라는 GUI 권고는 채택하지 않는다.",
      paragraphs: ["You also have to modify"] },
    { heading: "Offset", korean: "버튼 메모리의 배치와 충돌: 유닛의 버튼 데이터를 넣을 빈 영역의 포인터가 필요하다. 서로 다른 생성 작업의 데이터가 겹치면 기존 버튼과 충돌한다. 버튼 위치 정렬과 메모리 영역 분리는 별개의 제약이며, 같은 영역을 재사용해도 안전하다고 가정하면 안 된다.",
      paragraphs: ["It should be", "When you use"] },
    { heading: "Sorting", korean: "유닛의 버튼 정렬과 표시 위치: 버튼 목록은 position 순서로 정렬한다. 정렬하지 않으면 버튼이 작동하지 않거나 표시 위치가 잘못될 수 있다는 원문 설명이다. 다른 생성 작업의 버튼과 데이터가 겹치는 메모리 충돌은 Offset 절의 별도 제약이다." },
    { heading: "Redirecting", korean: "버튼 리다이렉트는 다른 유닛 버튼셋을 공유하는 것. Dat Requirements를 별도로 검토해야 하며 버튼 표시와 사용 가능 여부는 같지 않다.",
      quotes: ["It sets one unit's buttons to another unit's."] },
    { heading: "Changing unit command buttons dynamically", korean: "동적 버튼 리다이렉트: 편집한 버튼 데이터 없이 redirect만 생성하면 수정본이 아닌 원래 버튼을 가리킨다. 대상 버튼의 저장 위치 정보가 함께 필요하다.",
      paragraphs: ["You can set", "Just remember"] }
  ]
};

const eudtoolsReferencePaths = [
  "Data/iscriptopcodes.txt",
  "Data/iscriptanimations.txt",
  "Include/IscriptIDList.txt"
] as const;

// Manually read at EUDTOOLS_COMMIT. These are image observations, not tested code.
const eudtoolsImageReviews: Record<string, string> = {
  "DataEditor_IscriptID.jpg": "이미지 관찰: Images 'Shuttle'의 Script ID가 153 Dragoon으로 표시된다. CastSpell은 00000, 미리보기에는 No Frame이 보인다. GUI 클릭 절차가 아니라 기존 스크립트 선택과 프레임/슬롯 불일치의 예시다.",
  "HydraSkill.png": "이미지 관찰(EE3, 게임 미검증): Cast Disruption Web/Psionic Storm Animation=3, Use_Weapon_Targeting=1. Hydra의 spellCooldown 20–60이면 Ground/Air_Weapon=84, 70–160이면 101, 두 경우 cooldown=15. cooldown<=2이면 두 무기를 38로 복원한다. next unit 포인터 저장 뒤 Hydralisk를 생성한다. 애니메이션이 무기를 사용하기 전 교체하고 두 번째 공격 전 복원해야 하며, 수치는 이 예제 전용이다.",
  "DynamicCUnit.png": "이미지 관찰(EE3): 전역 DarkArchon 선언, onPluginStart에서 다음 생성 유닛 포인터를 DarkArchon에 저장한 다음 P1의 Protoss Dark Archon 1기를 create에 생성한다.",
  "TriggerSpell1.png": "이미지 관찰(EE3): DarkArchon buildQueue[1] Exactly 64 감지 → buildQueue[1]부터 [5] 모두 228, secondaryOrderID=2, Switch 3 Set. 큐 비우기와 생산 보조 명령 초기화를 함께 한다.",
  "TriggerSpell2.png": "이미지 관찰(EE3): beforeTriggerExec에서 DarkArchon spellCooldown AtLeast 80 → SetTo 0 → Raw Code `setloc(10, wread(DarkArchon + 0x58), wread(DarkArchon + 0x5A));` → Switch 3 Set. 0x58/0x5A는 X/Y이며, 이 원문 Raw Code를 현대 epScript API로 검증하거나 변환하지 않았다.",
  "defi61.jpg": "이미지 관찰: IscriptID 9 Defiler, Animation 61, CurrentOffset 10518. playfram 0x187, wait 1, attackwith 1, gotorepeatattk, ignorerest 등이 보이고 마지막 goto는 10319이다. Unused1 표시도 보이지만 정상 슬롯으로 해석하지 않는다. 역컴파일 화면일 뿐 실행 안전성 증거가 아니다.",
  "arbiterarchon.jpg": "이미지 관찰: Arbiter 위에 Archon 형상이 겹쳐 보인다. 정지 이미지로 공격 횟수나 누적 효과를 검증할 수 없으며 해당 효과는 본문의 주장으로만 보존한다."
};

const repositorySpecs = {
  scrmapdocs: {
    slug: "havonz/SCRMapDocs",
    url: "https://github.com/havonz/SCRMapDocs.git",
    sparsePaths: ["docs"]
  },
  eudplib: {
    slug: "armoha/eudplib",
    url: "https://github.com/armoha/eudplib.git",
    sparsePaths: ["docs", "src/eudplib", "tests"]
  },
  eudBook: {
    slug: "armoha/eud-book",
    url: "https://github.com/armoha/eud-book.git",
    sparsePaths: ["api.json", "docs/searchindex.json"]
  },
  editor: {
    slug: "Buizz/EUD-Editor-3",
    url: "https://github.com/Buizz/EUD-Editor-3.git",
    sparsePaths: [
      "EUD Editor 3/AvalonEdit/CodeEditor",
      "EUD Editor 3/Class/BulidData",
      "EUD Editor 3/Class/Data",
      "EUD Editor 3/Class/ExtraData",
      "EUD Editor 3/Class/TriggerEditor",
      "EUD Editor 3/Data/DatFiles",
      "EUD Editor 3/Data/TriggerEditor/epsFunctions_safe.txt",
      "EUD Editor 3/Module/Tools",
      "EUD Editor 3/Version.txt"
    ]
  }
} as const;

const editorContractPaths = [
  "EUD Editor 3/AvalonEdit/CodeEditor/TriggerEditorCompletionData.vb",
  "EUD Editor 3/Class/BulidData/WriteButtonData.vb",
  "EUD Editor 3/Class/BulidData/WriteDatFile.vb",
  "EUD Editor 3/Class/BulidData/WriteReqFile.vb",
  "EUD Editor 3/Class/BulidData/WriteTriggerEditor.vb",
  "EUD Editor 3/Class/BulidData/WriteedsFile.vb",
  "EUD Editor 3/Class/Data/CButtonData.vb",
  "EUD Editor 3/Class/Data/CRequireData.vb",
  "EUD Editor 3/Class/Data/ProgramData.vb",
  "EUD Editor 3/Class/Data/ProjectData/ProjectData.vb",
  "EUD Editor 3/Class/Data/SCDatFiles.vb",
  "EUD Editor 3/Class/ExtraData/ExtraDatFiles.vb",
  "EUD Editor 3/Class/TriggerEditor/TEFile.vb",
  "EUD Editor 3/Class/TriggerEditor/TriggerEditorData.vb",
  "EUD Editor 3/Module/Tools/BuildErrorHandling.vb"
] as const;

export type PublicSyncSummary = {
  outputFile: string;
  rows: number;
  commit: string;
};

type RepositorySpec = {
  slug: string;
  url: string;
  sparsePaths: readonly string[];
};

type RepositorySnapshot = {
  root: string;
  slug: string;
  commit: string;
};

type MarkdownSection = {
  key: string;
  title: string;
  content: string;
};

type PythonDefinition = {
  name: string;
  kind: "class" | "function";
  signature: string;
  documentation?: string;
  methods: string[];
};

type EudBookSearchIndex = {
  doc_urls: string[];
  index: {
    documentStore: {
      docs: Record<
        string,
        {
          body?: string;
          breadcrumbs?: string;
          title?: string;
        }
      >;
    };
  };
};

export async function syncPublicSources(
  outputDir = corpusOutputDir,
  selection: "all" | "eudtools" = "all"
): Promise<PublicSyncSummary[]> {
  const tempRoot = await mkdtemp(join(tmpdir(), "eud-agent-upstream-"));

  try {
    const eudtools = await clonePinnedBareSnapshot(
      "Ar3sgice/eudtools", EUDTOOLS_COMMIT, tempRoot, "eudtools.git"
    );
    const wiki = await clonePinnedBareSnapshot(
      "Ar3sgice/eudtools.wiki", EUDTOOLS_WIKI_COMMIT, tempRoot, "eudtools.wiki.git"
    );
    const eudtoolsRows = await buildEudtoolsRows(eudtools, wiki);
    const eudtoolsOutputs: Array<[string, CorpusJsonRow[], string]> = [
      [EUDTOOLS_WIKI_SOURCE, eudtoolsRows.wikiRows, wiki.commit],
      [EUDTOOLS_REFERENCE_SOURCE, eudtoolsRows.referenceRows, eudtools.commit]
    ];
    if (selection === "eudtools") {
      for (const [fileName, rows] of eudtoolsOutputs) {
        await writeCorpusJsonlAtomic(join(outputDir, fileName), rows);
      }
      await writeThirdPartyNotices(outputDir, []);
      return eudtoolsOutputs.map(([outputFile, rows, commit]) => ({
        outputFile, rows: rows.length, commit
      }));
    }
    const scr = await cloneSnapshot(repositorySpecs.scrmapdocs, tempRoot, "scrmapdocs");
    const eudplib = await cloneSnapshot(repositorySpecs.eudplib, tempRoot, "eudplib");
    const eudBook = await cloneSnapshot(repositorySpecs.eudBook, tempRoot, "eud-book");
    const editor = await cloneSnapshot(repositorySpecs.editor, tempRoot, "editor3");

    const scrRows = await buildScrMapDocsRows(scr);
    const { apiRows, exampleRows } = await buildEudplibRows(eudplib);
    const eudBookRows = await buildEudBookRows(eudBook);
    const editorRows = await buildEditorRows(editor);

    const outputs: Array<[string, CorpusJsonRow[], string]> = [
      [SCR_SOURCE, scrRows, scr.commit],
      [EUDPLIB_API_SOURCE, apiRows, eudplib.commit],
      [EUDPLIB_EXAMPLE_SOURCE, exampleRows, eudplib.commit],
      [EUD_BOOK_SOURCE, eudBookRows, eudBook.commit],
      [EDITOR_SOURCE, editorRows, editor.commit],
      ...eudtoolsOutputs
    ];

    for (const [fileName, rows] of outputs) {
      await writeCorpusJsonlAtomic(join(outputDir, fileName), rows);
    }

    await writeThirdPartyNotices(outputDir, [scr, eudplib, editor]);

    return outputs.map(([outputFile, rows, commit]) => ({
      outputFile,
      rows: rows.length,
      commit
    }));
  } finally {
    await rm(tempRoot, { recursive: true, force: true });
  }
}

async function cloneSnapshot(
  spec: RepositorySpec,
  tempRoot: string,
  folder: string
): Promise<RepositorySnapshot> {
  const root = join(tempRoot, folder);
  await runGit([
    "clone",
    "--depth",
    "1",
    "--filter=blob:none",
    "--sparse",
    spec.url,
    root
  ]);
  await runGit([
    "-C",
    root,
    "sparse-checkout",
    "set",
    "--skip-checks",
    ...spec.sparsePaths
  ]);
  const commit = (await runGit(["-C", root, "rev-parse", "HEAD"])).trim();

  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(`Unexpected commit returned for ${spec.slug}: ${commit}`);
  }

  return { root, slug: spec.slug, commit };
}

async function clonePinnedBareSnapshot(
  slug: string,
  commit: string,
  tempRoot: string,
  folder: string
): Promise<RepositorySnapshot> {
  const root = join(tempRoot, folder);
  await runGit(["init", "--bare", root]);
  await runGit([
    "-C", root, "fetch", "--depth", "1", "--filter=blob:none",
    `https://github.com/${slug}.git`, commit
  ]);
  const actual = (await runGit(["-C", root, "rev-parse", "FETCH_HEAD"])).trim();
  if (actual !== commit) {
    throw new Error(`Pinned snapshot mismatch for ${slug}: ${actual}`);
  }
  // No checkout: wiki paths contain ':' and are invalid Windows filenames.
  return { root, slug, commit };
}

async function readSnapshotBlob(snapshot: RepositorySnapshot, path: string): Promise<string> {
  return stripBom(await runGit(["-C", snapshot.root, "show", `${snapshot.commit}:${path}`]))
    .replace(/\r\n?/g, "\n");
}

function eudtoolsRow(
  snapshot: RepositorySnapshot, path: string, key: string, title: string,
  body: string, source: string, section: string
): CorpusJsonRow {
  const isWiki = snapshot.slug.endsWith(".wiki");
  const page = path.replace(/\.md$/, "");
  const url = isWiki
    ? `https://github.com/Ar3sgice/eudtools/wiki/${encodeURIComponent(page)}/${snapshot.commit}`
    : githubBlobUrl(snapshot, path);
  const experimental = source === EUDTOOLS_EXPERIMENTAL_SOURCE;
  const content = [
    EUDTOOLS_CAUTION,
    ...(experimental ? ["실험적·버전 의존: 헤더 밖 해석/미정의 동작이며 현행 저작 지침이나 안전한 구현법이 아니다."] : []),
    body,
    `원저자: Ar3sgice. 원문 언어: en. Snapshot: ${snapshot.commit}.`
  ].join("\n\n");
  const row = makeRow({
    id: `${snapshot.slug}:${path}#${key}`,
    title: `[eudtools] ${title}`,
    content, url, source, snapshot, repoPath: path,
    language: "en", scope: "Legacy StarCraft/Brood War EUD; SC:R behavior only where explicitly reported by the author"
  });
  // Preserve the existing 2000-character chunk contract. Caveats lead the body
  // so the runtime's 480-character discovery preview also exposes them.
  if (Array.from(`제목: ${row.title}\n\n${row.content}`).length > 2000) {
    throw new Error(`eudtools logical entry exceeds one index chunk: ${row.id}`);
  }
  return {
    ...row, author: "Ar3sgice", section_path: [page, section],
    original_url: isWiki
      ? `https://github.com/Ar3sgice/eudtools/wiki/${encodeURIComponent(page)}`
      : `https://github.com/Ar3sgice/eudtools/blob/${snapshot.commit}/${path}`,
    permission_basis: EUDTOOLS_PERMISSION,
    verification: "Source/image inspection only; no game-runtime validation",
    experimental
  };
}

export function extractEudtoolsWikiRows(
  markdown: string, page: string, snapshot: RepositorySnapshot
): CorpusJsonRow[] {
  const selections = eudtoolsWikiSelections[page];
  if (!selections) throw new Error(`Wiki page outside eudtools allowlist: ${page}`);
  const sections = splitMarkdownSections(markdown);
  const rows: CorpusJsonRow[] = [];
  const source = page === "EUD-Tutorial:-Extended-Animations"
    ? EUDTOOLS_EXPERIMENTAL_SOURCE : EUDTOOLS_WIKI_SOURCE;
  for (const selection of selections) {
    const section = sections.find((candidate) => candidate.title === selection.heading);
    if (!section) throw new Error(`Missing pinned wiki section ${page}: ${selection.heading}`);
    const text = section.content.replace(/^#{1,6}[^\n]*\n?/, "").trim();
    // A fenced block is indivisible. Long legacy examples are explicitly omitted,
    // not truncated or translated into purportedly runnable modern code.
    let blocks: string[] = text.match(/```[^\n]*\n[\s\S]*?```|[^\n]+(?:\n(?!\n|```)[^\n]+)*/g) ?? [];
    if (selection.paragraphs) {
      blocks = selection.paragraphs.map((prefix) => {
        const matches = blocks.filter((block) => block.startsWith(prefix));
        if (matches.length !== 1) throw new Error(`Ambiguous selected paragraph: ${page}/${prefix}`);
        return matches[0];
      });
    }
    if (selection.quotes) {
      blocks = selection.quotes.map((quote) => {
        if (!text.includes(quote)) throw new Error(`Missing selected quote: ${page}/${quote}`);
        return quote;
      });
    }
    const rendered = blocks.map((block) => {
      const imageMatch = block.match(/^!\[\]\(([^)]+)\)$/);
      if (imageMatch) {
        const imageName = new URL(imageMatch[1]).pathname.split("/").at(-1)!;
        const review = eudtoolsImageReviews[imageName];
        if (!review) throw new Error(`Image needs direct review before indexing: ${imageName}`);
        const imageUrl = `https://raw.githubusercontent.com/Ar3sgice/eudtools/${EUDTOOLS_COMMIT}/Include/Wiki/${imageName}`;
        return { text: `${review}\n이미지 원문: ${imageUrl}`, excerpt: "", image: imageUrl };
      }
      if (block.startsWith("```") && block.length > 550) {
        return {
          text: "편집 주: 이 절의 긴 레거시 트리거/역컴파일 코드는 생략했다. 전체 코드는 연결된 원문을 참조한다. 이 발췌는 완전한 구현 예제가 아니다.",
          excerpt: "", image: ""
        };
      }
      return { text: block, excerpt: block, image: "" };
    });
    let group: typeof rendered = [];
    let part = 0;
    const flush = () => {
      if (!group.length) return;
      const row = eudtoolsRow(
        snapshot, `${page}.md`, `${section.key}-${++part}`,
        `${selection.korean.split(":")[0]} — ${selection.heading} (${part})`,
        `용어·맥락(편집): ${selection.korean}\n\n원문 발췌 / 이미지 관찰:\n${group.map((item) => item.text).join("\n\n")}`,
        source, selection.heading
      );
      rows.push({
        ...row,
        excerpts: group.map((item) => item.excerpt).filter(Boolean),
        image_sources: group.map((item) => item.image).filter(Boolean)
      });
      group = [];
    };
    for (const block of rendered) {
      if (group.length && group.map((item) => item.text).join("\n\n").length + block.text.length > 1000) flush();
      group.push(block);
    }
    flush();
  }
  return rows;
}

export function extractEudtoolsReferenceRows(
  raw: string, path: string, snapshot: RepositorySnapshot
): CorpusJsonRow[] {
  if (!(eudtoolsReferencePaths as readonly string[]).includes(path)) {
    throw new Error(`Reference outside eudtools allowlist: ${path}`);
  }
  const text = stripBom(raw).replace(/\r\n?/g, "\n").trimEnd();
  const rows: CorpusJsonRow[] = [];
  const add = (key: string, title: string, body: string) => rows.push(
    eudtoolsRow(snapshot, path, key, title, body, EUDTOOLS_REFERENCE_SOURCE, key)
  );
  if (path.endsWith("iscriptopcodes.txt")) {
    const lines = text.split("\n").filter((line) => line.trim());
    if (lines.shift() !== "IceCC opcode name, opcode ID, parameters, opcode description") {
      throw new Error("Unexpected iscript opcode header");
    }
    for (const line of lines) {
      const match = line.match(/^(\S+)\s+(0x[0-9a-f]+) - /);
      if (!match) throw new Error(`Unrecognized complete opcode entry: ${line}`);
      add(match[2], `iscript opcode ${match[1]} ${match[2]} — 의미·인자`,
        `IceCC opcode name, opcode ID, parameters, opcode description\n\n${line}\n\n` +
        "원문의 unknown/hypothesised/crashes 표현과 인자 표기를 그대로 보존했다. ticks·pixels 등 단위를 바꾸지 않는다.");
    }
  } else if (path.endsWith("iscriptanimations.txt")) {
    const entries = text.split(/\n(?=\+\d{2} )/);
    for (const entry of entries) {
      const match = entry.match(/^\+(\d{2}) (\S+) - /);
      if (!match) throw new Error(`Unrecognized animation entry: ${entry}`);
      add(match[1], `iscript 애니메이션 슬롯 ${match[2]} +${match[1]}`,
        `${entry}\n\n+NN은 원문의 헤더 바이트 오프셋(십진 표기)이다. ` +
        "Orders.dat의 0부터 시작하는 애니메이션 번호와 혼동하지 않는다. 예: +22 CastSpell은 슬롯 7이다. Unknown/unused는 검증된 용도로 해석하지 않는다.");
    }
  } else {
    const names = text.split("\n");
    // Zero-based line indices are corroborated by explicit IDs in the selected wiki.
    if (names[9] !== "Defiler" || names[70] !== "Ghost" || names[158] !== "High Templar" ||
        names[276] !== "Shield Overlay" || names[386] !== "Acid Spores (6-9) Overlay") {
      throw new Error("Iscript ID list no longer matches the selected wiki's zero-based IDs");
    }
    for (let start = 0; start < names.length; start += 16) {
      const end = Math.min(start + 16, names.length);
      add(`ids-${start}-${end - 1}`, `iscript ID와 이름 ${start}–${end - 1}`,
        "기존 iscript 선택용 ID → 이름. 유닛 ID 목록이 아니다. 원문 줄 순서를 0부터 세며 Unknown·오탈자도 원문 그대로다.\n\n" +
        names.slice(start, end).map((name, index) => `${start + index}: ${name}`).join("\n") +
        "\n\nID가 존재해도 대상 이미지의 프레임·공격·CastSpell 호환성을 보장하지 않는다.");
    }
  }
  return rows;
}

async function buildEudtoolsRows(body: RepositorySnapshot, wiki: RepositorySnapshot) {
  const wikiRows: CorpusJsonRow[] = [];
  const referenceRows: CorpusJsonRow[] = [];
  for (const page of Object.keys(eudtoolsWikiSelections)) {
    wikiRows.push(...extractEudtoolsWikiRows(
      await readSnapshotBlob(wiki, `${page}.md`), page, wiki
    ));
  }
  for (const path of eudtoolsReferencePaths) {
    referenceRows.push(...extractEudtoolsReferenceRows(
      await readSnapshotBlob(body, path), path, body
    ));
  }
  const ids = new Set<string>();
  for (const row of [...wikiRows, ...referenceRows]) {
    if (ids.has(String(row.id))) throw new Error(`Duplicate eudtools id: ${row.id}`);
    ids.add(String(row.id));
  }
  return { wikiRows: sortRows(wikiRows), referenceRows: sortRows(referenceRows) };
}

async function runGit(args: string[]): Promise<string> {
  const { promise, resolve, reject } = Promise.withResolvers<string>();
  execFile(
    "git",
    args,
    {
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
      windowsHide: true
    },
    (error, stdout, stderr) => {
      if (error) {
        reject(
          new Error(
            `git ${args.join(" ")} failed: ${String(stderr).trim() || error.message}`
          )
        );
        return;
      }
      resolve(String(stdout));
    }
  );
  return promise;
}

async function buildScrMapDocsRows(
  snapshot: RepositorySnapshot
): Promise<CorpusJsonRow[]> {
  const docsRoot = join(snapshot.root, "docs");
  const paths = await listFiles(docsRoot, (path) => path.endsWith(".md"));
  const rows: CorpusJsonRow[] = [];

  for (const path of paths) {
    const repoPath = toRepoPath(snapshot.root, path);
    const markdown = stripBom(await readFile(path, "utf8"));
    rows.push(
      ...markdownRows({
        markdown,
        repoPath,
        snapshot,
        source: SCR_SOURCE,
        titlePrefix: "SCRMapDocs",
        scope: "epScript and standalone euddraft reference",
        language: "English"
      })
    );
  }

  return sortRows(rows);
}

async function buildEudplibRows(snapshot: RepositorySnapshot): Promise<{
  apiRows: CorpusJsonRow[];
  exampleRows: CorpusJsonRow[];
}> {
  const packageRoot = join(snapshot.root, "src", "eudplib");
  const initPath = join(packageRoot, "__init__.py");
  const initText = stripBom(await readFile(initPath, "utf8"));
  const version = initText.match(/__version__\s*=\s*["']([^"']+)["']/)?.[1] ?? "unknown";
  const pythonPaths = await listFiles(packageRoot, (path) => path.endsWith(".py"));
  const exports = new Set<string>(["eudplibVersion"]);

  for (const path of pythonPaths.filter((path) => basename(path) === "__init__.py")) {
    const text = stripBom(await readFile(path, "utf8"));
    for (const name of extractPythonExports(text)) {
      exports.add(name);
    }
  }

  const apiRows: CorpusJsonRow[] = [
    makeRow({
      id: `${snapshot.slug}:exports`,
      title: `[eudplib ${version}] Public API export catalog`,
      content: [
        `Snapshot commit: ${snapshot.commit}`,
        "Scope: Python eudplib public API. Do not paste Python syntax into an epScript file.",
        "Exported symbols:",
        [...exports].sort().join(", ")
      ].join("\n\n"),
      url: githubBlobUrl(snapshot, "src/eudplib/__init__.py"),
      source: EUDPLIB_API_SOURCE,
      snapshot,
      repoPath: "src/eudplib/__init__.py",
      version,
      language: "Python",
      scope: "eudplib public API"
    })
  ];

  for (const path of pythonPaths) {
    const text = stripBom(await readFile(path, "utf8"));
    const repoPath = toRepoPath(snapshot.root, path);
    for (const definition of extractPythonDefinitions(text)) {
      if (!exports.has(definition.name) || definition.name.startsWith("_")) {
        continue;
      }

      const details = [
        `Snapshot commit: ${snapshot.commit}`,
        `eudplib version: ${version}`,
        "Scope: Python eudplib public API. Do not paste Python syntax into an epScript file.",
        `${definition.kind === "class" ? "Class" : "Function"} signature:`,
        definition.signature
      ];
      if (definition.documentation) {
        details.push("Documentation:", definition.documentation);
      }
      if (definition.methods.length > 0) {
        details.push("Public method signatures:", definition.methods.join("\n"));
      }

      apiRows.push(
        makeRow({
          id: `${snapshot.slug}:${repoPath}#${definition.name}`,
          title: `[eudplib ${version}] ${definition.name}`,
          content: details.join("\n\n"),
          url: githubBlobUrl(snapshot, repoPath),
          source: EUDPLIB_API_SOURCE,
          snapshot,
          repoPath,
          version,
          language: "Python",
          scope: "eudplib public API"
        })
      );
    }
  }

  const docsRoot = join(snapshot.root, "docs");
  const docPaths = await listFiles(docsRoot, (path) => path.endsWith(".md"));
  for (const path of docPaths) {
    const repoPath = toRepoPath(snapshot.root, path);
    apiRows.push(
      ...markdownRows({
        markdown: stripBom(await readFile(path, "utf8")),
        repoPath,
        snapshot,
        source: EUDPLIB_API_SOURCE,
        titlePrefix: `eudplib ${version}`,
        scope: "Python eudplib maintained documentation",
        language: "Korean/English",
        version
      })
    );
  }

  const testsRoot = join(snapshot.root, "tests");
  const epsPaths = await listFiles(testsRoot, (path) => path.endsWith(".eps"));
  const exampleRows: CorpusJsonRow[] = [];
  for (const path of epsPaths) {
    const repoPath = toRepoPath(snapshot.root, path);
    const chunks = chunkByLines(stripBom(await readFile(path, "utf8")), 1500);
    chunks.forEach((chunk, index) => {
      exampleRows.push(
        makeRow({
          id: `${snapshot.slug}:${repoPath}#${index}`,
          title: `[eudplib ${version} epScript test] ${basename(path)} part ${index + 1}/${chunks.length}`,
          content: [
            `Snapshot commit: ${snapshot.commit}`,
            "Scope: official epScript compiler test/example.",
            chunk
          ].join("\n\n"),
          url: githubBlobUrl(snapshot, repoPath),
          source: EUDPLIB_EXAMPLE_SOURCE,
          snapshot,
          repoPath,
          version,
          language: "epScript",
          scope: "official compiler test/example"
        })
      );
    });
  }

  return {
    apiRows: sortRows(apiRows),
    exampleRows: sortRows(exampleRows)
  };
}

async function buildEudBookRows(
  snapshot: RepositorySnapshot
): Promise<CorpusJsonRow[]> {
  const repoPath = "docs/searchindex.json";
  const parsed = JSON.parse(
    stripBom(await readFile(join(snapshot.root, ...repoPath.split("/")), "utf8"))
  ) as EudBookSearchIndex;
  const docs = parsed.index.documentStore.docs;
  const rows: CorpusJsonRow[] = [];

  for (const id of Object.keys(docs).sort(compareNumericStrings)) {
    const doc = docs[id];
    const title = doc.title?.trim() ?? "";
    const body = doc.body?.trim() ?? "";
    if (!title || !body || id === "0") {
      continue;
    }

    const docUrl = parsed.doc_urls[Number.parseInt(id, 10)];
    if (!docUrl) {
      throw new Error(`eud-book search index is missing doc_urls[${id}]`);
    }

    rows.push(
      makeRow({
        id: `${snapshot.slug}:${id}`,
        title: `[eud-book] ${title}`,
        content: [
          `Snapshot commit: ${snapshot.commit}`,
          "Scope: StarCraft memory/offset reference.",
          doc.breadcrumbs?.trim(),
          body
        ]
          .filter((part): part is string => Boolean(part))
          .join("\n\n"),
        url: `https://armoha.github.io/eud-book/${docUrl}`,
        source: EUD_BOOK_SOURCE,
        snapshot,
        repoPath,
        language: "English",
        scope: "StarCraft memory/offset reference"
      })
    );
  }

  return sortRows(rows);
}

async function buildEditorRows(
  snapshot: RepositorySnapshot
): Promise<CorpusJsonRow[]> {
  const editorRoot = join(snapshot.root, "EUD Editor 3");
  const versionText = stripBom(await readFile(join(editorRoot, "Version.txt"), "utf8"));
  const version = versionText.split(/\r?\n/, 1)[0]?.trim() || "unknown";
  const rows: CorpusJsonRow[] = [];
  const datRoot = join(editorRoot, "Data", "DatFiles");
  const definitionPaths = await listFiles(datRoot, (path) => path.endsWith(".def"));

  for (const path of definitionPaths) {
    const repoPath = toRepoPath(snapshot.root, path);
    rows.push(
      ...parseDatDefinitionRows({
        text: stripBom(await readFile(path, "utf8")),
        datName: basename(path, ".def"),
        repoPath,
        snapshot,
        version
      })
    );
  }

  const autocompletePath =
    "EUD Editor 3/Data/TriggerEditor/epsFunctions_safe.txt";
  rows.push(
    ...parseEditorFunctionRows({
      text: stripBom(
        await readFile(join(snapshot.root, ...autocompletePath.split("/")), "utf8")
      ),
      repoPath: autocompletePath,
      snapshot,
      version
    })
  );

  for (const repoPath of editorContractPaths) {
    const path = join(snapshot.root, ...repoPath.split("/"));
    const text = stripBom(await readFile(path, "utf8"));
    const chunks = chunkByParagraphs(text, 1500);
    chunks.forEach((chunk, index) => {
      rows.push(
        makeRow({
          id: `${snapshot.slug}:${repoPath}#${index}`,
          title: `[EUD Editor ${version} source contract] ${basename(repoPath)} part ${index + 1}/${chunks.length}`,
          content: [
            `Snapshot commit: ${snapshot.commit}`,
            "Scope: EUD Editor 3 internal model/build contract. This is not epScript syntax.",
            chunk
          ].join("\n\n"),
          url: githubBlobUrl(snapshot, repoPath),
          source: EDITOR_SOURCE,
          snapshot,
          repoPath,
          version,
          language: "VB.NET",
          scope: "editor internal model/build contract"
        })
      );
    });
  }

  return sortRows(rows);
}

export function splitMarkdownSections(markdown: string): MarkdownSection[] {
  const lines = stripBom(markdown).replace(/\r\n?/g, "\n").split("\n");
  const sections: MarkdownSection[] = [];
  const headingStack: string[] = [];
  const seenKeys = new Map<string, number>();
  let buffer: string[] = [];
  let inFence = false;

  const flush = () => {
    const content = buffer.join("\n").trim();
    buffer = [];
    if (!hasMeaningfulMarkdown(content)) {
      return;
    }

    const title =
      headingStack.filter((heading) => heading.length > 0).join(" > ") || "Overview";
    const baseKey = slugify(title) || "overview";
    const occurrence = seenKeys.get(baseKey) ?? 0;
    seenKeys.set(baseKey, occurrence + 1);
    sections.push({
      key: occurrence === 0 ? baseKey : `${baseKey}-${occurrence + 1}`,
      title,
      content
    });
  };

  for (const line of lines) {
    if (/^\s*```/.test(line) || /^\s*~~~/.test(line)) {
      inFence = !inFence;
      buffer.push(line);
      continue;
    }

    const heading = inFence
      ? undefined
      : line.match(/^\s*(?:-\s+)?(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (!heading) {
      buffer.push(line);
      continue;
    }

    flush();
    const level = heading[1].length;
    const title = cleanHeading(heading[2]);
    headingStack.length = Math.min(headingStack.length, level - 1);
    headingStack[level - 1] = title;
    buffer.push(line);
  }

  flush();
  return sections;
}

function markdownRows(options: {
  markdown: string;
  repoPath: string;
  snapshot: RepositorySnapshot;
  source: string;
  titlePrefix: string;
  scope: string;
  language: string;
  version?: string;
}): CorpusJsonRow[] {
  return splitMarkdownSections(options.markdown).map((section) =>
    makeRow({
      id: `${options.snapshot.slug}:${options.repoPath}#${section.key}`,
      title: `[${options.titlePrefix}] ${section.title}`,
      content: [
        `Snapshot commit: ${options.snapshot.commit}`,
        `Scope: ${options.scope}.`,
        `Source language: ${options.language}.`,
        section.content
      ].join("\n\n"),
      url: githubBlobUrl(options.snapshot, options.repoPath),
      source: options.source,
      snapshot: options.snapshot,
      repoPath: options.repoPath,
      version: options.version,
      language: options.language,
      scope: options.scope
    })
  );
}

export function extractPythonExports(text: string): string[] {
  const exports = new Set<string>();
  const assignmentPattern = /__all__\s*=\s*\[([\s\S]*?)\]/g;
  for (const assignment of text.matchAll(assignmentPattern)) {
    for (const literal of assignment[1].matchAll(/["']([A-Za-z_][A-Za-z0-9_]*)["']/g)) {
      exports.add(literal[1]);
    }
  }
  return [...exports].sort();
}

export function extractPythonDefinitions(text: string): PythonDefinition[] {
  const lines = stripBom(text).replace(/\r\n?/g, "\n").split("\n");
  const definitions: PythonDefinition[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^(async\s+def|def|class)\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (!match) {
      continue;
    }

    const headerEnd = findPythonHeaderEnd(lines, index);
    const signature = lines
      .slice(index, headerEnd + 1)
      .map((line) => line.trim())
      .join(" ");
    const blockEnd = findPythonBlockEnd(lines, headerEnd + 1);
    const documentation = extractPythonDocstring(lines, headerEnd + 1, blockEnd);
    const methods = match[1] === "class" ? extractClassMethods(lines, headerEnd + 1, blockEnd) : [];

    definitions.push({
      name: match[2],
      kind: match[1] === "class" ? "class" : "function",
      signature,
      documentation,
      methods
    });
    index = blockEnd - 1;
  }

  return definitions;
}

function findPythonHeaderEnd(lines: string[], start: number): number {
  for (let index = start; index < lines.length; index += 1) {
    if (lines[index].trimEnd().endsWith(":")) {
      return index;
    }
  }
  return start;
}

function findPythonBlockEnd(lines: string[], start: number): number {
  for (let index = start; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim().length > 0 && !/^\s/.test(line)) {
      return index;
    }
  }
  return lines.length;
}

function extractPythonDocstring(
  lines: string[],
  start: number,
  end: number
): string | undefined {
  let index = start;
  while (index < end && (lines[index].trim() === "" || lines[index].trimStart().startsWith("#"))) {
    index += 1;
  }
  if (index >= end) {
    return undefined;
  }

  const trimmed = lines[index].trim();
  const match = trimmed.match(/^[rRuUbBfF]*("""|''')/);
  if (!match) {
    return undefined;
  }

  const quote = match[1];
  const collected: string[] = [];
  let remainder = trimmed.slice(match[0].length);
  const sameLineEnd = remainder.indexOf(quote);
  if (sameLineEnd >= 0) {
    return remainder.slice(0, sameLineEnd).trim() || undefined;
  }
  if (remainder.length > 0) {
    collected.push(remainder);
  }

  for (index += 1; index < end; index += 1) {
    const line = lines[index].trim();
    const closing = line.indexOf(quote);
    if (closing >= 0) {
      if (closing > 0) {
        collected.push(line.slice(0, closing));
      }
      break;
    }
    collected.push(line);
  }

  const documentation = collected.join("\n").trim();
  return documentation || undefined;
}

function extractClassMethods(lines: string[], start: number, end: number): string[] {
  const candidates: Array<{ indent: number; index: number }> = [];
  for (let index = start; index < end; index += 1) {
    const match = lines[index].match(/^(\s+)(?:async\s+def|def)\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (match && !match[2].startsWith("_")) {
      candidates.push({ indent: match[1].length, index });
    }
  }
  if (candidates.length === 0) {
    return [];
  }

  const methodIndent = Math.min(...candidates.map((candidate) => candidate.indent));
  return candidates
    .filter((candidate) => candidate.indent === methodIndent)
    .map((candidate) => {
      const headerEnd = findPythonHeaderEnd(lines, candidate.index);
      return lines
        .slice(candidate.index, headerEnd + 1)
        .map((line) => line.trim())
        .join(" ");
    });
}

export function parseDatDefinition(text: string): Array<Record<string, string>> {
  const parameters = new Map<string, Record<string, string>>();
  let inFormat = false;

  for (const rawLine of stripBom(text).replace(/\r\n?/g, "\n").split("\n")) {
    const line = rawLine.trim();
    if (line === "[FORMAT]") {
      inFormat = true;
      continue;
    }
    if (!inFormat || line.length === 0 || line.startsWith("[")) {
      continue;
    }

    const match = line.match(/^(\d+)([A-Za-z][A-Za-z0-9]*)=(.*)$/);
    if (!match) {
      continue;
    }
    const record = parameters.get(match[1]) ?? { index: match[1] };
    record[match[2]] = match[3].trim();
    parameters.set(match[1], record);
  }

  return [...parameters.values()].sort(
    (left, right) => Number.parseInt(left.index, 10) - Number.parseInt(right.index, 10)
  );
}

function parseDatDefinitionRows(options: {
  text: string;
  datName: string;
  repoPath: string;
  snapshot: RepositorySnapshot;
  version: string;
}): CorpusJsonRow[] {
  return parseDatDefinition(options.text).map((parameter) => {
    const name = parameter.Name || `parameter ${parameter.index}`;
    const details = Object.entries(parameter)
      .filter(([key]) => key !== "Name")
      .map(([key, value]) => `${key}: ${value}`)
      .join("\n");
    return makeRow({
      id: `${options.snapshot.slug}:${options.repoPath}#${parameter.index}`,
      title: `[EUD Editor ${options.version}/${options.datName}.dat] ${name}`,
      content: [
        `Snapshot commit: ${options.snapshot.commit}`,
        "Scope: exact EUD Editor 3 DAT parameter schema. Use the parameter name verbatim with dat_get/dat_set.",
        `DAT table: ${options.datName}`,
        `Parameter name: ${name}`,
        details
      ].join("\n\n"),
      url: githubBlobUrl(options.snapshot, options.repoPath),
      source: EDITOR_SOURCE,
      snapshot: options.snapshot,
      repoPath: options.repoPath,
      version: options.version,
      language: "EUD Editor DAT definition",
      scope: "editor DAT schema"
    });
  });
}

export function parseEditorFunctions(text: string): Array<{
  name: string;
  documentation: string;
  signature: string;
}> {
  const functions: Array<{
    name: string;
    documentation: string;
    signature: string;
  }> = [];
  const pattern = /\/\*\*\*([\s\S]*?)\*\/\s*(function\s+([A-Za-z_][A-Za-z0-9_]*)[^\n{]*\{\})/g;

  for (const match of stripBom(text).matchAll(pattern)) {
    functions.push({
      name: match[3],
      documentation: match[1]
        .split(/\r?\n/)
        .map((line) => line.replace(/^\s*\*+\s?/, ""))
        .join("\n")
        .trim(),
      signature: match[2].trim()
    });
  }

  return functions;
}

function parseEditorFunctionRows(options: {
  text: string;
  repoPath: string;
  snapshot: RepositorySnapshot;
  version: string;
}): CorpusJsonRow[] {
  return parseEditorFunctions(options.text).map((entry) =>
    makeRow({
      id: `${options.snapshot.slug}:${options.repoPath}#${entry.name}`,
      title: `[EUD Editor ${options.version} epScript API] ${entry.name}`,
      content: [
        `Snapshot commit: ${options.snapshot.commit}`,
        "Scope: epScript function advertised by the current EUD Editor 3 autocomplete data.",
        entry.signature,
        entry.documentation
      ].join("\n\n"),
      url: githubBlobUrl(options.snapshot, options.repoPath),
      source: EDITOR_SOURCE,
      snapshot: options.snapshot,
      repoPath: options.repoPath,
      version: options.version,
      language: "epScript",
      scope: "editor-provided epScript API"
    })
  );
}

function makeRow(options: {
  id: string;
  title: string;
  content: string;
  url: string;
  source: string;
  snapshot: RepositorySnapshot;
  repoPath: string;
  version?: string;
  language: string;
  scope: string;
}): CorpusJsonRow {
  return {
    id: options.id,
    title: options.title,
    content: options.content.trim(),
    url: options.url,
    source: options.source,
    commit: options.snapshot.commit,
    language: options.language,
    path: options.repoPath,
    repo: options.snapshot.slug,
    scope: options.scope,
    ...(options.version ? { version: options.version } : {})
  };
}

async function listFiles(
  root: string,
  predicate: (path: string) => boolean
): Promise<string[]> {
  const files: string[] = [];
  const walk = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        await walk(path);
      } else if (entry.isFile() && predicate(path)) {
        files.push(path);
      }
    }
  };
  await walk(root);
  return files;
}

function toRepoPath(root: string, path: string): string {
  return relative(root, path).split(sep).join("/");
}

function githubBlobUrl(snapshot: RepositorySnapshot, repoPath: string): string {
  const encodedPath = repoPath
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
  return `https://github.com/${snapshot.slug}/blob/${snapshot.commit}/${encodedPath}`;
}

function sortRows(rows: CorpusJsonRow[]): CorpusJsonRow[] {
  return [...rows].sort((left, right) =>
    String(left.id ?? left.url ?? left.title).localeCompare(
      String(right.id ?? right.url ?? right.title),
      "en"
    )
  );
}

function compareNumericStrings(left: string, right: string): number {
  return Number.parseInt(left, 10) - Number.parseInt(right, 10);
}

function stripBom(text: string): string {
  return text.replace(/^\uFEFF/, "");
}

function cleanHeading(value: string): string {
  return value
    .replace(/\[([^\]]+)\]\([^\)]+\)/g, "$1")
    .replace(/[\*`_]/g, "")
    .replace(/<[^>]+>/g, "")
    .trim();
}

function slugify(value: string): string {
  return value
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "");
}

function hasMeaningfulMarkdown(value: string): boolean {
  return value
    .replace(/<br\s*\/?\s*>/gi, "")
    .replace(/<!--([\s\S]*?)-->/g, "")
    .trim().length > 0;
}

function chunkByLines(text: string, maxChars: number): string[] {
  const chunks: string[] = [];
  let current: string[] = [];
  let length = 0;

  for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
    const addition = line.length + (current.length > 0 ? 1 : 0);
    if (current.length > 0 && length + addition > maxChars) {
      chunks.push(current.join("\n").trim());
      current = [];
      length = 0;
    }
    current.push(line);
    length += addition;
  }
  if (current.length > 0) {
    chunks.push(current.join("\n").trim());
  }
  return chunks.filter((chunk) => chunk.length > 0);
}

function chunkByParagraphs(text: string, maxChars: number): string[] {
  const paragraphs = text.replace(/\r\n?/g, "\n").split(/\n{2,}/);
  const chunks: string[] = [];
  let current = "";

  for (const paragraph of paragraphs) {
    const clean = paragraph.trim();
    if (!clean) {
      continue;
    }
    if (clean.length > maxChars) {
      if (current) {
        chunks.push(current);
        current = "";
      }
      chunks.push(...chunkByLines(clean, maxChars));
      continue;
    }
    const candidate = current ? `${current}\n\n${clean}` : clean;
    if (candidate.length > maxChars) {
      chunks.push(current);
      current = clean;
    } else {
      current = candidate;
    }
  }
  if (current) {
    chunks.push(current);
  }
  return chunks;
}

async function writeThirdPartyNotices(
  outputDir: string,
  snapshots: RepositorySnapshot[]
): Promise<void> {
  const sections: string[] = [
    "Third-party source snapshots embedded in the RAG corpus.",
    "Generated by tools/scraper/src/publicSources.ts."
  ];

  for (const snapshot of snapshots) {
    const licensePath = join(snapshot.root, "LICENSE");
    const license = stripBom(await readFile(licensePath, "utf8"));
    sections.push(
      [
        "================================================================================",
        `${snapshot.slug} @ ${snapshot.commit}`,
        `https://github.com/${snapshot.slug}`,
        "--------------------------------------------------------------------------------",
        license.trim()
      ].join("\n")
    );
  }

  const noticePath = join(outputDir, "THIRD_PARTY_NOTICES.txt");
  const marker = "================================================================================\nAr3sgice/eudtools @ ";
  if (!snapshots.length) {
    // Selective refresh must retain notices for the seven untouched corpora.
    // A missing notice is an error, never permission to silently discard attribution.
    const existing = stripBom(await readFile(noticePath, "utf8"));
    const separator = `\n\n${"=".repeat(80)}\n`;
    const retained = existing.split(separator)
      .filter((section) => !section.startsWith("Ar3sgice/eudtools @ "))
      .join(separator).trimEnd();
    sections.splice(0, sections.length, retained);
  }
  sections.push([
    `${marker}${EUDTOOLS_COMMIT}`,
    `Ar3sgice/eudtools.wiki @ ${EUDTOOLS_WIKI_COMMIT}`,
    "Original author: Ar3sgice",
    "https://github.com/Ar3sgice/eudtools",
    "https://github.com/Ar3sgice/eudtools/wiki",
    "--------------------------------------------------------------------------------",
    EUDTOOLS_PERMISSION,
    "No upstream license text is supplied for these snapshots; no MIT or other license is inferred.",
    "Selected wiki paths (only these six documents):",
    ...Object.keys(eudtoolsWikiSelections).map((page) => `  ${page}.md`),
    "Selected reference paths (only these three documents):",
    ...eudtoolsReferencePaths.map((path) => `  ${path}`),
    "Linked images were visually inspected for textual descriptions only, at the body commit:",
    ...Object.keys(eudtoolsImageReviews).map((name) => `  Include/Wiki/${name}`),
    "Images, binaries, JavaScript sources, EUDDB and other wiki pages are not index inputs.",
    "Original English excerpts and separately labeled Korean editorial context are retained.",
    "Legacy examples and experimental/version-dependent claims are not current verified authoring APIs."
  ].join("\n"));

  await writeTextAtomic(
    join(outputDir, "THIRD_PARTY_NOTICES.txt"),
    `${sections.join("\n\n")}\n`
  );
}

async function writeTextAtomic(path: string, content: string): Promise<void> {
  const tmpPath = `${path}.tmp`;
  await writeFile(tmpPath, content, "utf8");
  try {
    await rename(tmpPath, path);
  } catch (error) {
    await unlink(tmpPath).catch(() => undefined);
    throw error;
  }
}
