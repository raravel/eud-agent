# EUD Agent — 설치 안내

EUD Editor 3에서 자연어로 epScript(eps) 코드를 생성·적용해 주는 AI 에이전트입니다.
에디터 본체는 수정하지 않으며, 파일 복사 방식으로만 설치됩니다.

## 사전 준비물

설치 전에 아래 2가지가 필요합니다.
(PowerShell은 Windows 내장 버전으로 충분합니다 — 별도 설치 불필요)

1. **uv** (Python 환경 관리 도구)
   - https://docs.astral.sh/uv/ 의 안내를 따르거나, 터미널에서:
     `winget install astral-sh.uv`
2. **codex CLI + 계정** (BYO — 본인 계정 사용)
   - Node.js가 있다면: `npm install -g @openai/codex`
   - Node.js가 없다면 https://nodejs.org 에서 설치 후 위 명령 실행
   - 설치 후 `codex login` 으로 로그인해 두세요

> 참고: 첫 사용 시 임베딩 모델(bge-m3, 약 4.3 GB)이 자동 다운로드됩니다.
> 디스크 여유 공간과 네트워크 상황을 감안해 주세요.

## 설치

1. 이 압축을 **임의의 폴더에 풀어 주세요** (한 번 정하면 옮기지 마세요 —
   설치 시 이 폴더의 절대 경로가 에디터 설정에 기록됩니다).
2. `scripts\install.bat` 을 더블클릭합니다.
3. EUD Editor 3 설치 폴더 경로를 물어보면 입력합니다.
   (예: `C:\EUD Editor 3` — `EUD Editor 3.exe` 가 있는 폴더)
4. 설치가 끝나면 Enter 로 창을 닫고 에디터를 실행합니다.

**주의: 설치/제거 전에 EUD Editor 3을 반드시 종료해 주세요.**
에디터가 켜져 있으면 DLL 파일이 잠겨 설치가 실패합니다.

## 사용

에디터에서 프로젝트를 열면 에이전트 패널 창이 자동으로 뜹니다.
지시문을 입력하고 대상 파일을 고른 뒤, 생성된 코드를 확인하고 Apply 하면
에디터의 해당 파일에 반영됩니다(저장은 에디터에서 직접).

## 제거

`scripts\uninstall.bat` 을 더블클릭하고 에디터 경로를 입력하면
브리지 파일과 에이전트 런타임 데이터가 제거됩니다.

## 폴더 구성

```
eud-agent\
├── bridge\            에디터에 복사되는 브리지 스크립트
├── server\            로컬 Python 서버 (설치 시 .venv 생성)
├── panel\dist\        에이전트 패널 UI
├── vendor\webview2\   WebView2 런타임 DLL
├── rag\chromadb_bge\  ECA 예제 검색 DB (동봉)
└── scripts\           install.bat / uninstall.bat
```

## 알려진 제한

- Windows 전용, 에디터 1개 인스턴스만 지원
- SET/적용은 메모리상 변경이며 저장은 에디터에서 직접 해야 합니다
