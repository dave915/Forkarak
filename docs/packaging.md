# macOS 패키징

`scripts/package-macos.sh`는 저장소의 루트 경로를 자신의 위치에서 계산한다. 작업 디렉터리와 저장소 경로에 공백이나 한글이 있어도 인자를 각각 전달한다. macOS 기본 Bash 3.2에서 동작하도록 작성했다.

## 명령과 산출물

| 명령 | 동작 |
| --- | --- |
| `./scripts/package-macos.sh` | arm64 release 빌드 후 번들 생성 |
| `./scripts/package-macos.sh --debug` | arm64 debug 빌드 후 번들 생성 |
| `./scripts/package-macos.sh --run` | 번들 생성 후 macOS `open`으로 실행 |
| `./scripts/package-macos.sh --binary /path/to/pokarak` | 기존 arm64 바이너리 패키징. Cargo 호출 없음 |
| `./scripts/package-macos.sh --help` | 사용법 출력. 빌드 없음 |

`--run`은 `--debug` 또는 `--binary`와 함께 쓸 수 있다. `--debug`와 `--binary`는 함께 쓰지 않는다. `--binary`의 상대 경로는 명령을 실행한 디렉터리를 기준으로 한다.

```text
dist/Pokarak.app/
  Contents/
    Info.plist
    PkgInfo
    MacOS/pokarak
    Resources/
      Pokarak.icns
      spork.svg
      LICENSE
      fonts/                 TTF, OFL 원문, 출처, 체크섬
    _CodeSignature/          로컬 ad-hoc 서명
```

메타데이터 원본은 `assets/macos/Info.plist`다. 이름 `Pokarak`, 식별자 `dev.pokarak.app`, 최소 OS `13.0`, 선호 아키텍처 `arm64`를 선언한다. 버전은 패키징할 때 `Cargo.toml`의 `workspace.package.version`으로 갱신한다. 실행 파일은 `pokarak`이다. 앱의 창 제목은 별도로 앱 코드에서 지정한다. CLI 설치·업데이트 스크립트도 `Contents/Resources/`에 포함된다.

## 빌드와 교체 과정

1. `~/.cargo/bin`을 PATH에 추가하고 메타데이터와 아이콘을 확인한다.
2. `--binary`가 없으면 `MACOSX_DEPLOYMENT_TARGET=13.0`으로 `cargo build --locked --package pokarak --bin pokarak --target aarch64-apple-darwin`을 호출한다. 기본은 `--release`이며 `--debug`는 dev 프로파일을 사용한다.
3. `cargo metadata --no-deps`의 `target_directory`를 사용한다. 따라서 `CARGO_TARGET_DIR` 또는 Cargo 설정의 target 디렉터리를 반영한다.
4. 입력 바이너리의 실행 권한과 `lipo -archs`를 확인한다. Intel 전용 및 universal 바이너리는 거부한다.
5. `dist/` 아래 임시 디렉터리에서 새 번들을 구성하고 서명·서명 검증을 한다. 기본은 ad-hoc이며 `SIGNING_IDENTITY`가 있으면 hardened runtime과 timestamp를 포함한 서명을 사용한다.
6. 기존 출력이 Pokarak 번들인지 확인한 뒤 교체한다. 새 번들이 준비되기 전에 기존 앱을 지우지 않으며, 교체 중 오류가 나면 기존 번들의 복원을 시도한다. 출력 경로가 심볼릭 링크면 중단한다.
7. `--run`이면 `open`으로 번들을 실행한다.

`--binary`는 기존 바이너리를 다시 컴파일하지 않는다. 해당 바이너리의 최소 OS, 외부 라이브러리 의존성, 리소스 요구사항은 빌드를 담당한 쪽에서 확인해야 한다. 번들 폰트는 런타임 폰트 등록을 대신하지 않는다. 이 스크립트는 별도의 외부 dylib 수집을 수행하지 않는다.

## 검증 범위

패키징 작업에서는 전체 앱 빌드를 실행하지 않았다. 확인한 항목은 다음과 같다.

- `/bin/bash -n`으로 두 셸 스크립트 문법 확인
- 빌드 없는 CLI 경로 5개 확인: 도움말, 알 수 없는 옵션, 누락된 바이너리 인자, 충돌 옵션, 존재하지 않는 바이너리
- `plutil -lint`와 필드 검사로 Info.plist 구문·메타데이터 확인
- AppKit SVG 렌더링, `iconutil` ICNS 생성 후 역변환으로 10개 이미지 크기 확인
- 원본 TTF 파일 형식과 다운로드 SHA-256 확인
- 앱 아이콘 PNG 육안 확인

실제 앱 바이너리가 준비되면 다음 명령으로 번들과 실행을 확인한다.

```sh
./scripts/package-macos.sh --binary /absolute/path/to/pokarak
plutil -lint "dist/Pokarak.app/Contents/Info.plist"
lipo -archs "dist/Pokarak.app/Contents/MacOS/pokarak"
codesign --verify --strict --verbose=2 "dist/Pokarak.app"
open "dist/Pokarak.app"
```

Dock/Finder의 이름·아이콘, 한글 표시, 폰트 굵기와 코드 서체, 실제 저장소 열기를 별도로 확인한다. 화면 검증 항목은 DESIGN.md를 따른다. 기본 생성 결과는 ad-hoc 서명이며 Apple 공증이 없다. GitHub 릴리스용 압축, CLI 설치·업데이트, 선택적 Developer ID 서명·공증 경로는 [배포 문서](distribution.md)를 참고한다. DMG 생성과 앱 내부 자동 업데이트는 지원하지 않는다.
