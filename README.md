# 포카락 · Pokarak

![포카락 스포크 마크](assets/icons/spork.svg)

로컬 Git 워크트리의 변경사항과 주변 코드를 읽기 위한 **네이티브 macOS 앱**이다. Rust와 GPUI + GPUI Kit를 사용하며 웹뷰를 사용하지 않는다. 제품 방향은 [PRODUCT.md](PRODUCT.md), 화면 규칙은 [DESIGN.md](DESIGN.md)에 있다.

## 현재 상태

이 문서는 초기 scaffold와 자산·패키징 작업을 기준으로 작성했다. 다른 워크트리의 기능 통합 및 앱 실기동 검증 결과는 아직 반영하지 않았다.

| 항목 | 현재 확인한 범위 |
| --- | --- |
| 앱 소스 | 1440×900 초기 창, 제품명과 안내 문구를 표시하는 GPUI scaffold |
| Git·검색 | 공개 타입과 함수 틀만 존재. 현재 scaffold의 백엔드는 미초기화 오류를 반환 |
| 언어 분석 | `ServerConfig`와 탐색 API 틀만 존재. 기본 서버 목록은 비어 있음 |
| 자산 | 원본 스포크 SVG, 실제 ICNS 10종 해상도, 공식 TTF 6개와 라이선스 고지 |
| 패키징 | Apple Silicon `.app` 생성 스크립트 작성. Bash 문법과 Info.plist 검증 완료 |
| 통합 실행 | 이 작업에서는 Cargo 앱 빌드, 실제 `.app` 생성·실행, 화면 동작을 검증하지 않음 |

구현 목표는 워크트리 탐색과 전환, HEAD 또는 선택한 기준 브랜치와의 비교, 변경 파일·통합 diff·전체 파일 읽기, 파일·텍스트 검색, 뒤로/앞으로 이동, 명시적으로 시작하는 언어 분석, 외부 변경 알림이다. 워크트리별 읽기 위치 복원과 실제 저장소 데이터 사용도 목표에 포함된다. 이 목록은 완료된 기능 목록이 아니다.

## 개발 실행

Apple Silicon Mac, Xcode command-line tools, Rust/Cargo가 필요하다. 패키징 메타데이터의 최소 macOS는 13.0이며 이 버전에서 실제 실행 검증은 아직 하지 않았다. 첫 Cargo 빌드는 의존성 다운로드와 컴파일에 시간이 걸릴 수 있다.

저장소 루트에서:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo run -p pokarak
```

`cargo`를 찾지 못하면 `~/.cargo/bin/cargo --version`으로 설치 위치를 확인한다. 개발 도구 설치 상태는 `xcode-select -p`로 확인할 수 있다. 현재 scaffold에는 저장소 경로를 받는 CLI 옵션이 없다.

## macOS 앱 만들기

기본 명령은 `aarch64-apple-darwin` release 바이너리를 빌드하고 `dist/포카락.app`을 만든 뒤 실행한다.

```sh
./scripts/package-macos.sh --run
```

개발용 debug 빌드:

```sh
./scripts/package-macos.sh --debug --run
```

이미 만든 바이너리로 패키징만 할 수도 있다. 이 옵션은 Cargo를 호출하지 않는다.

```sh
./scripts/package-macos.sh --binary /absolute/path/to/pokarak --run
```

직접 만든 기본 target 바이너리라면 `--binary target/debug/pokarak`처럼 지정한다. 입력은 arm64 단일 아키텍처의 실행 파일이어야 한다. 재실행은 다음과 같다.

```sh
open "dist/포카락.app"
```

번들 ID는 `dev.pokarak.app`, 표시 이름은 `포카락`, 버전은 `0.1.0`이다. ICNS와 폰트·라이선스를 함께 복사하고 로컬 개발용 ad-hoc 서명을 한다. Developer ID 서명·공증·DMG 배포는 포함하지 않는다. 세부 구조와 검증 명령은 [패키징 문서](docs/packaging.md)를 참고한다.

여러 워크트리에서 `/Users/han/dev/pokarak/target`을 공유한다면 `CARGO_TARGET_DIR`를 지정할 수 있다. Cargo 잠금 때문에 빌드가 직렬화될 수 있으므로 다른 빌드가 끝난 뒤 실행한다. 이 캐시를 삭제하지 않는다.

```sh
CARGO_TARGET_DIR=/Users/han/dev/pokarak/target ./scripts/package-macos.sh --debug
```

Herdr에서 긴 빌드는 설치된 `hc --skill` 지침대로 실행한다.

```sh
hc run --bg --name pokarak-package ./scripts/package-macos.sh --debug
hc wait pokarak-package --timeout 60
hc read pokarak-package --lines 40
```

## 언어 분석과 저장소 신뢰

파일 열기와 diff 읽기는 언어 서버 없이 동작하도록 설계한다. 언어 서버와 프로젝트 import·빌드는 저장소 신뢰 확인 및 사용자의 명시적 분석 요청 이후에만 허용한다. 텍스트 검색 후보를 의미 기반 참조로 표시하지 않는다.

현재 scaffold에는 설정 파일 로더, 신뢰 승인 UI, 동작하는 언어 서버 연결이 없다. 서버 설치나 JSON 파일 생성만으로 분석이 활성화되지 않는다. 공개 설정 타입, 향후 연결 시 확인할 항목과 실행 범위는 [언어 분석 문서](docs/language-analysis.md)에 설명했다.

## 알려진 제한

- 기능 통합 후 실제 저장소와 다중 워크트리에서 검증해야 한다. 현재 문서는 탐색·검색·diff·상태 복원을 실행 검증된 기능으로 보증하지 않는다.
- Java/Kotlin 및 JS/TS 언어 서버의 호환성·메모리·응답 속도, 과거 버전 코드 탐색은 아직 검증하지 않았다.
- Git 쓰기, 저장소 편집, 고급 히스토리 편집, Spring 전용 분석은 현재 범위에 없다.
- Intel Mac, 다른 OS, 최소 지원 macOS, 서명된 외부 배포는 검증하지 않았다.
- UI 상태는 검사 대상 저장소 밖에 저장해야 한다. scaffold에는 영속 저장 구현이나 확정된 저장 경로가 없다.
- 폰트 파일은 제공하지만 GPUI 등록과 한글 fallback은 앱 통합 후 확인해야 한다.

## 자산과 라이선스

원본 스포크 마크는 [assets/icons/spork.svg](assets/icons/spork.svg), 앱 아이콘은 [assets/icons/app-icon.svg](assets/icons/app-icon.svg)다. `./scripts/build-icons.sh`로 macOS AppKit과 `iconutil`만 사용해 ICNS를 재생성할 수 있다.

UI용 IBM Plex Sans와 코드용 JetBrains Mono의 파일 목록, 공식 출처, 라이선스, 체크섬은 [폰트 문서](assets/fonts/README.md)에 있다. 프로젝트 코드와 자체 제작 자산은 [MIT](LICENSE), 동봉 서체는 각 SIL Open Font License 1.1을 따른다.
