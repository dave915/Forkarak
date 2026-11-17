# 포카락 · Pokarak

AI가 바꾼 코드를 여러 프로젝트와 Git 워크트리에서 읽는 Rust 네이티브 macOS 앱.
GPUI + GPUI Kit로 만들며 웹뷰를 사용하지 않는다.

## 설치 · 업데이트

Apple Silicon Mac(M1 이상), macOS 13 이상에서 터미널로 설치한다. Rust, Xcode, Homebrew, GitHub 로그인 없이 빌드된 앱을 받는다.

```sh
/bin/bash -o pipefail -c 'curl -fsSL https://github.com/dave915/pokarak/releases/latest/download/install.sh | /bin/bash'
```

앱은 `~/Applications/포카락.app`, CLI는 `~/.local/bin/pokarak`에 설치된다. 설치가 끝나면 Finder에서 앱을 열거나 다음 명령을 사용한다.

```sh
~/.local/bin/pokarak /path/to/project
~/.local/bin/pokarak --version
# 앱을 종료한 뒤 최신 릴리스로 업데이트
~/.local/bin/pokarak update
```

`~/.local/bin`이 PATH에 있으면 `pokarak`, `pokarak update`로 줄여 쓴다. 필요하면 `export PATH="$HOME/.local/bin:$PATH"`를 셸 설정에 추가한다. 설치 프로그램은 셸 설정을 바꾸지 않는다.

다운로드 SHA-256, 앱 식별자와 서명을 검사하고 교체한다. 설치 경로 설정과 세션 데이터는 업데이트 후에도 유지한다. 현재 배포는 ad-hoc 서명으로 **Apple 공증이 없으며**, 모든 Mac에서 보안 경고 없이 실행됨을 보장하지 않는다. [설치 옵션·릴리스 배포](docs/distribution.md)

## 실행

빌드된 앱: `dist/포카락.app`. Finder에서 더블클릭하거나 다음 명령으로 연다.

```sh
open "dist/포카락.app"
```

개발 실행에는 Apple Silicon Mac, Xcode, Rust가 필요하다.

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo run -p pokarak -- /path/to/project
# 여러 프로젝트를 같은 창에서 열기
cargo run -p pokarak -- /path/to/backend /path/to/frontend
```

## 구현된 기능

- 기본 화면은 커밋 그래프다. 위에서 커밋을 선택하고 아래에서 변경 파일과 diff·커밋 시점의 코드를 읽는다. 상단 **코드 보기**에서 기존 작업 파일 화면으로 전환하며, 프로젝트·워크트리를 열면 그래프부터 표시한다.
- 상단바 왼쪽에서 프로젝트와 그래프·코드 보기를 전환한다. 오른쪽 Git 툴바는 좁은 창에서도 Stage / 커밋, 브랜치, Stash, Fetch, Pull, Push, Git 작업 더 보기를 아이콘으로 표시한다. 아이콘에 마우스를 올리면 기능명이 나타난다.
- 상단바의 터미널 아이콘을 누르면 현재 워크트리에서 macOS 터미널이 열린다. 새 대화형 로그인 zsh가 포카락이 받은 환경변수를 상속하고 사용자의 zsh 설정을 읽으므로 설치된 AI CLI를 바로 실행할 수 있다.
- 저장소의 main/linked worktree 발견과 보기 전환. 전환은 checkout을 실행하지 않는다.
- 미커밋 변경: HEAD와 현재 파일 비교. staged·unstaged 변경을 함께 확인한다.
- 브랜치 변경: 선택한 기준 브랜치와의 공통 조상부터 현재 파일까지 비교한다. 이미 커밋된 변경도 포함한다.
- 변경 파일 목록, 전체 파일 트리, 파일명 필터, 프로젝트 본문 검색.
- 변경 파일에서 **좌우**를 선택하면 변경 전후 코드가 줄 번호와 함께 정렬된다. 작업 파일과 그래프에서 선택한 커밋에 모두 적용된다.
- 파일의 **History**에서 이름 변경 전 이력과 커밋별 diff를 열고, **Blame**에서 각 줄의 작성자와 커밋을 확인한다. 작업 중 바뀐 줄에는 작업 파일 표시가 붙는다.
- 읽기 전용 코드, 구문 강조, 파일 탭, 워크트리별 검색·선택·읽기 위치 저장, 뒤로/앞으로.
- `⌘클릭`: 참조에서 정의로 이동. 선언에서는 사용처 표시. 별도 사용처/구현체 명령.
- 외부 변경 알림. 갱신 전 읽던 화면을 유지하고, 오래된 코드 위치로 의미 탐색을 요청하지 않는다.
- 분석 서버는 명시적으로 허용한 워크트리에서 필요할 때 시작한다. 전환·닫기·종료 시 소유 프로세스를 취소한다.
- 경계 없는 macOS 제목 표시줄, 기본 창 제어 버튼, 크기 조절 패널, 번들 폰트와 포카락 아이콘.

### Git 작업

- **Stage / 커밋**에서 작업 파일이나 Staged 파일을 선택하고, 변경 줄 또는 블록 머리글을 눌러 **선택 n줄 Stage / Unstage**를 실행한다. **좌우**에서 선택한 쪽의 변경 전후를 비교할 수 있다. 바이너리, 특수 Git 필터 및 대용량 파일은 파일 단위로 처리한다.
- 사이드바의 **워크트리 관리**에서 새 브랜치 또는 기존 브랜치로 워크트리를 생성한다. 연결된 워크트리의 제거는 브랜치를 보존한다. 이미 사라진 경로의 등록도 개별 정리할 수 있다.
- 브랜치를 우클릭하거나 **Git 작업 → 브랜치 통합**에서 Merge 또는 Rebase를 선택한다. 확인창에서 현재 브랜치, 대상 브랜치, 두 커밋을 확인한 뒤 실행한다. 충돌은 **Stash / 충돌 해결**에서 계속하거나 중단한다.
- **Git 그래프**에서 `⌘F`로 커밋 메시지·작성자·짧은 해시를 검색한다. 왼쪽의 워크트리·브랜치·태그·Stash를 클릭하면 해당 커밋으로 이동한다. 브랜치에 포인터를 두면 첫 번째 부모 경로가 강조된다.
- 그래프 아래에서 부모별 diff를 보고, 커밋을 우클릭해 cherry-pick과 revert를 실행한다. **미커밋 변경** 행에서는 줄·블록을 Stage하거나 Unstage하고 메시지를 작성해 커밋한다. 상세 패널을 접거나 경계선을 드래그하면 워크트리마다 접힘 상태와 크기를 기억한다.
- **Git → Git 작업 및 저장소 설정…** 에서 브랜치 생성·체크아웃·삭제, fetch·push·fast-forward pull, 원격 저장소와 로컬 작성자 이름·이메일 설정을 제공한다.
- **파일 → Stash / 충돌 해결…** 에서 변경사항을 보관하거나 복원하고, 충돌 파일을 해결한 뒤 진행 중인 Git 작업을 계속하거나 중단한다. [Stash·충돌 해결 세부 동작](docs/git-workflow.md)

## 코드 탐색 준비

파일과 diff는 분석 서버 없이 동작한다. 정의·사용처 탐색을 준비하려면 다음 설치를 **직접 한 번** 실행한다.

```sh
./scripts/setup-language-servers.sh
```

설치 위치는 `~/.local/share/pokarak/tools`다. 별도 JDK 25, 공식 JetBrains Kotlin LSP, JDT LS, Node, TypeScript 서버, Maven을 준비한다. 프로젝트의 소스 수준이나 전역 `JAVA_HOME`을 바꾸지 않는다. 앱 시작 때 다운로드하지 않는다.

앱에서 **코드 탐색 → 신뢰하고 분석 허용**을 선택한다. Maven/Gradle import는 프로젝트 플러그인이나 빌드 설정을 실행할 수 있다. 최초 분석은 프로젝트 크기와 의존성 다운로드에 따라 시간이 걸린다.

| 탐색 범위 | 검증 상태 |
| --- | --- |
| JS/TS/JSX/TSX 정의, TS 사용처·구현체 | 실제 TypeScript 서버 테스트 통과 |
| Java → Java 정의·사용처 | 실제 JDT LS, Java 17 소스 테스트 통과 |
| Kotlin 정의·사용처 | 실제 공식 Kotlin LSP, Maven 테스트 통과 |
| Kotlin → Java 정의 | 혼합 Maven 프로젝트에서 통과 |
| Kotlin 심볼의 사용처에 Java 호출 포함 | 혼합 Maven 프로젝트에서 통과 |
| Java → Kotlin 정의, Java 심볼의 Kotlin 호출자 | 완전한 양방향 지원 미확립. JDT 사용처 결과는 Java 범위임을 표시 |
| Gradle, 대규모 사내 멀티모듈, 생성 코드, Spring 특화 분석 | 별도 실프로젝트 검증 필요 |

설정 덮어쓰기는 `~/.config/pokarak/language-servers.json`에서 할 수 있다. 상세 스키마·실제 검증 결과·외부 소스 제한은 [언어 분석 문서](docs/language-analysis.md)와 [navigation README](crates/navigation/README.md)를 참고한다.

## 단축키

| 동작 | macOS |
| --- | --- |
| 프로젝트 열기 | ⌘O |
| 프로젝트 전환 | Control-Tab / Control-Shift-Tab |
| 워크트리 터미널 열기 | Control + 백틱 |
| 워크트리 전환 | ⌘⌥↓ / ⌘⌥↑ |
| 파일 찾기 | ⌘P, ↑↓, Enter |
| 본문 검색 | ⌘⇧F, 검색어, Enter |
| 정의 이동 / 사용처 | ⌘클릭 / ⌥F7 |
| 정의 이동 | ⌘B |
| 뒤로 / 앞으로 | ⌘[ / ⌘] |
| diff·코드 전환 | ⌘D |
| 파일 닫기 / 프로젝트 닫기 | ⌘W / ⌘⇧W |
| 새로고침 | ⌘R |
| Stage / 커밋 | ⌘⇧K |
| 커밋 그래프 / 기록 | ⌘⌥H |
| Stash / 충돌 해결 | ⌘⇧G |

검색 결과 패널에서 ↑↓와 Enter로 결과를 연다. 모든 프로젝트 닫기는 파일 메뉴에 있다.

## 로컬 상태와 제한

프로젝트·워크트리의 UI 상태는 `~/.config/pokarak/session.json`에 저장한다. 분석용 임시 인덱스는 검사 대상 저장소 밖에 둔다. 언어 서버가 사용하는 일반 Maven/Gradle 캐시는 해당 도구 정책을 따른다.

Git 읽기는 외부 diff/filter/hook을 실행하지 않는다. 파일당 텍스트 미리보기는 2 MiB, diff는 50,000행 및 계산 제한이 있다. 제한을 넘으면 부분 표시를 알린다. 통합 diff의 긴 줄은 가로 스크롤로 변경 전·후 내용을 모두 확인할 수 있다. 바이너리·비 UTF-8 텍스트는 코드로 표시하지 않는다. 자세한 비교/rename/symlink 처리 범위는 [core README](crates/core/README.md)에 있다.

macOS Apple Silicon에서 빌드·실행했다. Intel Mac, Windows/Linux 패키징, 최소 macOS 13 환경, Developer ID 서명·공증은 검증하지 않았다. CLI 업데이트는 명시적으로 `pokarak update`를 실행할 때만 동작한다.

## 빌드와 검사

```sh
cargo test --workspace --features pokarak-navigation/test-server
cargo clippy -p pokarak --all-targets -- -D warnings
# 명시적으로 설치한 실제 언어 서버로 재검증
cargo test -p pokarak-navigation --test jvm -- --ignored --test-threads=1
cargo test -p pokarak-navigation --test typescript -- --ignored
# 실행용 앱 만들기
./scripts/package-macos.sh
```

Herdr에서는 오래 걸리는 빌드와 검사를 `hc run --bg`로 시작하고 `hc wait`로 기다린다. 패키징 상세는 [docs/packaging.md](docs/packaging.md)에 있다.

UI 검증 시 `--window-size 940x580 --session-dir /tmp/pokarak-qa`를 사용하면 기본 설정을 건드리지 않는 별도 세션으로 시작할 수 있다.

## 라이선스

프로젝트 코드는 [MIT](LICENSE). 원본 마크는 [spork.svg](assets/icons/spork.svg), ICNS 원본은 [app-icon.svg](assets/icons/app-icon.svg)다. IBM Plex Sans와 JetBrains Mono의 출처·OFL 고지·체크섬은 [폰트 문서](assets/fonts/README.md)에 있다.
