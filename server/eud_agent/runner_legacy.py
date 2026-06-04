#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""EUD Editor 3 외부 에이전트 러너 (codex 백엔드, 사용자 부담 LLM).

설계 원칙(메모리 agent-port-architecture): 무거운 LLM 로직은 외부 프로세스가 맡고
Lua 브리지는 얇게. 이 러너가 그 외부 프로세스다.

흐름:
  패널이 큐에 작업을 남김  →  러너가 받아서
    1) ECA RAG(rag_query.py)로 관련 eps/eud3 지식 검색(컨텍스트)
    2) codex -p 로 epScript 코드 생성 (LLM 비용은 사용자 codex 계정 부담)
    3) 브리지 inbox 에 'SET <대상파일>\\n<코드>' 명령을 써서 에디터에 반영
  (새 파일 생성이 필요하면 브리지에 AGENT/NEWEPS 명령 추가가 필요 — 하단 주석 참고)

큐 레이아웃 (에디터 설치 Data\\agent\\ 기준):
  jobs\\<id>.json   : {"instruction": "...", "target": "트리/파일/경로", "context": true}
  처리 후 jobs\\<id>.json -> jobs\\<id>.done, inbox\\agent_<id>.cmd 생성

사용:
  python eud_agent_runner.py --once [--mock] [--no-context] [--agent-dir PATH]
  python eud_agent_runner.py            # 상시 폴링
환경변수:
  CODEX_CMD   codex 호출 커맨드(기본 'codex exec'). 프롬프트가 마지막 인자로 붙음.
  EUD_AGENT_DIR  에디터 Data\\agent 경로 오버라이드.
"""
import sys
import os
import re
import json
import time
import shlex
import argparse
import subprocess

try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
VENV_PY = os.path.join(SCRIPT_DIR, ".venv", "Scripts", "python.exe")
RAG = os.path.join(SCRIPT_DIR, "rag_query.py")
DEFAULT_AGENT_DIR = os.environ.get(
    "EUD_AGENT_DIR",
    r"C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0\Data\agent")
CODEX_CMD = shlex.split(os.environ.get("CODEX_CMD", "codex exec"))

SYSTEM = (
    "너는 스타크래프트 EUD 맵 제작용 epScript(eps) 코드를 작성하는 어시스턴트다. "
    "아래 [참고자료]는 네이버 카페/공식 매뉴얼에서 검색한 eps/eud3 지식이다. "
    "사용자 요청을 만족하는 epScript 코드만 출력해라. 설명/마크다운 없이 코드만. "
    "플레이어 루프·변수 선언 등 eps 관례를 지켜라."
)


# ----------------------------------------------------------- 컨텍스트(RAG)
def retrieve_context(instruction, n=5):
    """rag_query.py 의미검색 stdout을 컨텍스트로 반환(실패 시 빈 문자열)."""
    py = VENV_PY if os.path.exists(VENV_PY) else sys.executable
    try:
        r = subprocess.run([py, RAG, instruction, str(n)],
                           capture_output=True, text=True, encoding="utf-8", timeout=180)
        return (r.stdout or "").strip()
    except Exception as e:
        print(f"[rag] 컨텍스트 검색 실패: {e}", file=sys.stderr)
        return ""


# ----------------------------------------------------------- codex 호출
def run_codex(prompt, mock=False):
    if mock:
        return ("// [mock] codex 미설치 — 더미 epScript\n"
                "function afterTriggerExec() {\n"
                "    foreach(p : EUDLoopPlayer()) {\n"
                "        setdeaths(p, SetTo, 1, \"Terran Marine\");\n"
                "    }\n}\n")
    proc = subprocess.run(CODEX_CMD + [prompt], capture_output=True, text=True,
                          encoding="utf-8", timeout=600)
    if proc.returncode != 0:
        raise RuntimeError(f"codex exit={proc.returncode}: {(proc.stderr or '')[:300]}")
    return proc.stdout


_FENCE = re.compile(r"```[a-zA-Z0-9_]*\n(.*?)```", re.S)


def extract_code(text):
    """codex 출력에서 코드블록만 추출(없으면 전체)."""
    blocks = _FENCE.findall(text or "")
    if blocks:
        return "\n\n".join(b.strip() for b in blocks).strip()
    return (text or "").strip()


# ----------------------------------------------------------- 적용(브리지 inbox)
def apply_via_bridge(agent_dir, job_id, target, code):
    inbox = os.path.join(agent_dir, "inbox")
    os.makedirs(inbox, exist_ok=True)
    cmd = f"SET {target}\n{code}"
    path = os.path.join(inbox, f"agent_{job_id}.cmd")
    with open(path, "w", encoding="utf-8") as f:
        f.write(cmd)
    return path


# ----------------------------------------------------------- 작업 처리
def process_job(agent_dir, jobpath, mock=False, use_context=True):
    job_id = os.path.splitext(os.path.basename(jobpath))[0]
    with open(jobpath, encoding="utf-8") as f:
        job = json.load(f)
    instruction = job.get("instruction", "").strip()
    target = job.get("target", "").strip()
    want_ctx = use_context and job.get("context", True)
    print(f"[job {job_id}] instruction={instruction!r} target={target!r} ctx={want_ctx}", file=sys.stderr)

    ctx = retrieve_context(instruction) if want_ctx else ""
    prompt = f"{SYSTEM}\n\n[참고자료]\n{ctx}\n\n[요청]\n{instruction}\n\n[epScript 코드]"
    raw = run_codex(prompt, mock=mock)
    code = extract_code(raw)

    cmd_path = apply_via_bridge(agent_dir, job_id, target, code)
    os.replace(jobpath, jobpath.replace(".json", ".done"))
    print(f"[job {job_id}] 적용됨 → {cmd_path}  (코드 {len(code)}B)", file=sys.stderr)
    return cmd_path, code


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--agent-dir", default=DEFAULT_AGENT_DIR)
    ap.add_argument("--once", action="store_true", help="대기열 1회 처리 후 종료")
    ap.add_argument("--mock", action="store_true", help="codex 대신 더미 코드")
    ap.add_argument("--no-context", action="store_true", help="RAG 컨텍스트 비활성")
    a = ap.parse_args()
    jobs = os.path.join(a.agent_dir, "jobs")
    os.makedirs(jobs, exist_ok=True)
    print(f"[runner] agent_dir={a.agent_dir} mock={a.mock} once={a.once}", file=sys.stderr)
    while True:
        for fn in sorted(os.listdir(jobs)):
            if fn.endswith(".json"):
                try:
                    process_job(a.agent_dir, os.path.join(jobs, fn),
                                mock=a.mock, use_context=not a.no_context)
                except Exception as e:
                    print(f"[job {fn}] ERROR: {e}", file=sys.stderr)
        if a.once:
            break
        time.sleep(1.0)


if __name__ == "__main__":
    main()


# ===========================================================================
# 브리지(ZZZ_10_agent_bridge.lua) 측 추가 제안 — 부모가 통합할 부분(아직 미적용):
#   1) PANEL 에 'AI 에이전트' 섹션: TextBox(지시) + Button.
#      btn.Click: 지시문을 받아 Data\agent\jobs\<n>.json 파일로 기록
#        {"instruction": txt, "target": "<선택/새 파일 경로>", "context": true}
#      (러너가 그 job을 처리해 inbox 로 SET 명령을 되돌려줌 → 기존 SET 핸들러가 반영)
#   2) 새 파일 생성이 필요하면: 러너가 SET 대신 쓸 수 있도록 브리지에
#      'NEWEPS <파일명>\n<코드>' inbox 명령을 추가(현재 PANEL 버튼 로직 재사용).
#   장점: LLM 비용은 사용자 codex 부담(BYO), 위험한 호출은 외부 프로세스에 격리
#         (.NET 예외가 pcall 관통해 에디터를 죽이는 문제와 무관).
# ===========================================================================
