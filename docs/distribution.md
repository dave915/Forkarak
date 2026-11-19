# 설치, CLI와 배포

지원 대상은 Apple Silicon Mac, macOS 13 이상이다. 네이티브 Rust/GPUI 앱이며 Intel Mac과 Windows/Linux 배포는 검증하지 않았다. 최소 OS는 선언값으로, macOS 13 실기기 검증과는 구분한다.

이 문서는 Forkarak **1.0.7**의 에디터·언어 관리·CLI 기능과 공개 배포 절차를 설명한다.

## 현재 소스에서 빌드하기

개발 Mac에는 Rust/Cargo와 Apple 개발 도구·SDK가 필요하다. 다음 명령은 프로젝트 루트에서 실행한다.

```sh
cargo run -p pokarak -- /absolute/path/to/git-project
./scripts/package-macos.sh
```

첫 명령은 개발 실행 파일을 사용한다. 두 번째 명령은 `dist/Forkarak.app`을 만들며 자동 실행하지 않는다. 기본 패키지에는 Java 서버와 런타임·고지·소스 자료가 포함된다. 패키징 머신에는 Python 3, 네이티브 Liquid Glass 스택을 검사할 수 있는 macOS 26의 `/usr/bin/assetutil`과 콜드 캐시 다운로드용 네트워크가 필요하다. 아이콘은 저장소의 컴파일된 카탈로그를 검증해 사용한다. 설치 후 앱 시작이나 관리 서버 사용에 사용자 Python/Homebrew/npm 설치가 필요한 것은 아니다. Herdr에서 오래 걸리는 빌드·검증은 `hc run --bg`와 `hc wait`로 수행한다.

앱을 원하는 위치에 둔 뒤, 그 **현재 번들**에서 CLI를 설치한다. 예를 들어 `~/Applications/Forkarak.app`에 둔 경우:

```sh
~/Applications/Forkarak.app/Contents/MacOS/forkarak install-cli
# 다른 명령 설치 폴더를 쓰려면:
~/Applications/Forkarak.app/Contents/MacOS/forkarak install-cli --bin-dir /absolute/path/to/bin
```

`install-cli`는 이 로컬 번들을 검증하고 `~/.local/bin/forkarak`과 `~/.local/bin/fkk`를 함께 설치한다. 앱을 다운로드하거나 교체하지 않으며 셸 설정을 변경하지 않는다. 두 명령은 해당 번들의 위치를 사용하므로 앱을 이동했다면 새 위치에서 다시 설치한다. `~/.local/bin`을 PATH에 추가하면 짧은 명령으로 실행할 수 있다.

## 파일과 프로젝트 열기

```sh
fkk .
forkarak /absolute/path/to/git-project
fkk src/order-service.ts
fkk --goto src/order-service.ts:24:5
forkarak --reuse-window /absolute/path/to/another-git-project
forkarak --new-window .
```

`forkarak`과 `fkk`는 같은 기능이다. 경로는 **기존 Git 워크트리 안의 존재하는 파일·디렉터리**여야 한다. Git 밖의 단독 파일, 새 파일 생성, `--wait`와 `--diff`는 지원하지 않는다. 파일을 열기 위해 git-init, checkout, 빌드나 언어 분석을 실행하지 않는다. 예약어 또는 `-`로 시작하는 파일은 `--` 뒤에 쓴다.

| 옵션 | 동작 |
| --- | --- |
| `-g, --goto FILE:LINE[:COLUMN]` | 여러 번 지정 가능. 줄·열은 1부터 시작하며 열은 Unicode 문자 기준이다. 전체 이름과 일치하는 파일이 있으면 그 이름을 우선한다. 일반 경로의 콜론은 위치로 해석하지 않는다. |
| `-r, --reuse-window` | 일치하는 실행 세션을 재사용하며, 명시적 재사용이면 유일한 다른 세션에 프로젝트를 열 수도 있다. 대상이 모호하면 오류로 표시한다. |
| `-n, --new-window` | 새 창으로 연다. `--reuse-window`와 함께 쓸 수 없다. |
| `--session SOCKET` | 재사용할 로컬 세션을 명시한다. |
| `--background`, `--no-launch` | 동일한 자동화 옵션. 실행 중인 적격 세션만 사용하며 앱을 실행하거나 앞으로 가져오지 않는다. 세션이 없으면 실패한다. |
| `--session-dir DIR` | 설정·세션 경로와 세션 매칭 범위를 분리한다. |
| `--window-size WIDTHxHEIGHT` | 새 창의 크기를 지정한다. 기존 창 크기는 바꾸지 않는다. |

기본 실행은 모든 요청 워크트리가 들어 있는 유일한 세션을 재사용하고, 파일·위치가 준비된 뒤 해당 창을 보여준다. 여러 세션이 일치하면 임의로 고르지 않는다. 자동화에서는 `--background`와 명시적 세션을 사용한다. 위치 검증은 화면에 표시한 실제 소스와 보관 중인 초안을 기준으로 한다. 전체 문법은 `forkarak --help`, JSON 자동화는 `forkarak ctl --help`를 참고한다.

## 공개 안정 버전 설치와 업데이트

공개 GitHub Releases의 최신 안정 버전을 설치한다.

```sh
/bin/bash -o pipefail -c 'curl -fsSL https://github.com/dave915/Forkarak/releases/latest/download/install.sh | /bin/bash'
~/.local/bin/forkarak --version
~/.local/bin/forkarak /path/to/project
```

기본 경로는 `~/Applications/Forkarak.app`이며 `~/.local/bin/forkarak`과 `~/.local/bin/fkk`를 함께 설치한다. `sudo`나 Xcode, Rust, Homebrew, GitHub 계정이 필요하지 않다. 설치 프로그램은 앱을 자동으로 실행하거나 셸 설정을 수정하지 않는다. `~/.local/bin`을 PATH에 추가하면 `forkarak`으로 실행할 수 있다.

이전 `Pokarak.app` 사용자도 같은 명령으로 Forkarak을 새로 설치할 수 있다. 기본 경로를 변경하려면 `--app-dir`와 `--bin-dir`을 사용한다.

설치된 앱은 실행 중 새 정식 릴리스를 확인한다. 새 버전 알림이나 설정에서 **업데이트 다운로드**를 누르면 현재 앱을 계속 사용하는 동안 패키지를 받아 검증한다. **업데이트하고 재시작**을 누르면 실행 중인 앱과 같은 위치에 설치하고 이전 워크트리를 다시 연다.

터미널에서 업데이트하려면 앱을 종료하고 다음 명령을 실행한다. 설치 때 지정한 앱·명령 경로를 다시 사용한다.

```sh
~/.local/bin/forkarak update
```

설치 프로그램은 공개 릴리스 페이지에서 태그를 확인한다. 네트워크 오류가 있으면 실패한 단계와 URL을 표시한다.

설치 스크립트를 내려받아 옵션을 지정할 수도 있다. `--version`은 버전을 고정할 때 사용하며 해당 GitHub 릴리스가 있어야 한다.

```sh
installer="$(mktemp)"
curl -fsSL https://github.com/dave915/Forkarak/releases/latest/download/install.sh -o "$installer" && \
  /bin/bash "$installer" --version v1.0.7 --app-dir "$HOME/Applications" --bin-dir "$HOME/.local/bin"
rm -f "$installer"
```

앱에서 재시작하면 세션 설정에 현재 작업 위치와 작성 중인 커밋 메시지를 저장한다. 저장소 파일과 Git 인덱스는 그대로 유지된다. 추가 언어 도구는 [언어 지원 관리](language-analysis.md)에서 명시적으로 설치하며 프로젝트 분석 허용은 별도로 관리한다.

## 다운로드와 교체

설치 프로그램은 최신 릴리스 태그를 한 번 결정한 뒤 같은 태그에서 앱과 `SHA256SUMS`를 받는다. SHA-256, `dev.pokarak.app` 식별자, 버전, arm64 실행 파일, 코드 서명을 검사한 뒤 설치한다. SHA-256과 ad-hoc 서명은 파일 손상 검출에 쓰며 Apple의 개발자 신원 인증을 뜻하지 않는다.

검증에 실패하면 설치된 앱을 교체하지 않는다. 관련 없는 앱이나 명령, 심볼릭 링크를 덮어쓰지 않는다. CLI로 교체하는 동안 앱이 실행 중이면 종료 후 재시도를 안내한다. 세부 검사와 오류 경로는 `scripts/test-distribution.py`에서 임시 앱과 격리된 설치 디렉터리로 검증한다.

## Java 리소스와 검증

기본 패키지의 `Contents/Resources/language-tools/`에는 JDT LS 1.63.0, Temurin 25.0.5+8, 전체 대응 OpenJDK 소스·빌드 자료, JDT와 중첩 JAR의 고지·소스 목록이 들어 있다. 약 **486 MiB**의 언어 리소스이며 다운로드 압축 크기와는 다르다. Kotlin은 바이너리 없이 공식 첫 사용 다운로드 메타데이터만 포함한다.

`language-resources.lock.json`의 고정 URL·해시를 사용하고 `~/Library/Caches/Forkarak/language-resources`의 캐시도 매번 검증한다. 준비된 리소스는 `FORKARAK_LANGUAGE_RESOURCES`로 지정할 수 있다. 빌드 단계에서 내부 파일 링크를 검증·정규화하고 경로 탈출·특수 파일·위험한 링크는 거부한다. JAR 원본은 유지하고 파일시스템의 중첩 Mach-O와 JDK, 외부 앱 순서로 서명한 뒤 최종 `SHA256SUMS` 인벤토리를 만든다.

설치 프로그램은 macOS 시스템 도구로 리소스 목록·무결성과 앱 서명을 확인한다. 현재 설치 경로에서는 앱과 두 관리 CLI를 함께 검증·게시하며, 도중 실패하면 이전 앱·명령을 복원한다. 관련 없는 파일·심볼릭 링크는 덮어쓰지 않는다. 소스의 MIT 라이선스와 포함된 런타임·제3자 자산의 라이선스는 별개다. 실제 고지와 소스 접근 정보는 [번들 고지](../scripts/language-resources-NOTICE.md)와 패키지의 `legal/`, `sources/`에 있다.

번들은 네이티브 아이콘 카탈로그 `Assets.car`와 이전 macOS용 `.icns` 대체 리소스를 함께 포함한다. macOS 26 이상에서는 카탈로그의 이름별 이미지를 사용하며 이전 시스템이나 해당 이름을 불러올 수 없는 경우 고정 PNG를 사용한다. **기본값 따르기**는 번들의 기본 그림을 따르고, 명시적으로 선택한 여섯 색상은 번들 밖 앱 공통 설정에 저장해 실행 시 Dock 아이콘에 복원한다. 색상별 Finder 아이콘 교체 기능을 뜻하지 않는다. 패키징은 카탈로그와 대체 리소스의 해시·원본 일치를 검사하며, 불일치 시 중단한다.

## 새 버전 게시 절차 (유지보수용)

아래 `v1.0.7`은 1.0.7 릴리스의 예시다. 이후 릴리스도 실제 게시 버전과 `Cargo.toml`의 workspace 버전을 반드시 일치시켜야 한다. 릴리스 스크립트는 불일치하면 거부한다.

`.github/workflows/release.yml`은 `vX.Y.Z` 태그를 push하면 GitHub 호스팅 `macos-26`에서 아이콘 자산·설치·CLI·언어 리소스 테스트, 포맷·Clippy·앱 테스트, arm64 release 빌드·패키징과 Java 번들의 설치·이동 검증을 수행한다. `scripts/test-icon-assets.py`와 패키징 검사는 호스트의 `/usr/bin/assetutil`로 네이티브 스택을 검사하며, 배포 대상 `MACOSX_DEPLOYMENT_TARGET=13.0`은 유지한다. 성공한 산출물만 새 릴리스로 게시한다. 같은 태그의 기존 릴리스 파일은 자동으로 덮어쓰지 않는다. GitHub 문서의 [최신 릴리스 다운로드 URL](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases)을 사용하므로 설치 명령은 버전마다 바뀌지 않는다.

1. 기능 변경을 검증하고 통합한다.
2. `Cargo.toml`의 workspace 버전과 `Cargo.lock`의 로컬 패키지 버전을 갱신하고 커밋한다.
3. 해당 커밋에 같은 버전의 태그를 붙이고 원격으로 보낸다.

```sh
git tag v1.0.7
git push origin HEAD v1.0.7
```

워크플로 수동 실행은 이미 존재하는 태그를 받는다. 실패한 빌드를 고친 경우 새 버전을 사용하고, 기존 릴리스가 없는 단순 일시 오류는 같은 태그의 워크플로를 재실행한다.

로컬 산출물은 `dist/release/`에 생기며, 각 GitHub 릴리스에는 다음 파일이 있다.

| 파일 | 내용 |
| --- | --- |
| `Forkarak-macos-arm64.zip` | Finder에서 압축을 풀어 사용하는 `Forkarak.app` |
| `forkarak-macos-arm64.tar.gz` | Forkarak.app과 CLI 리소스가 들어 있는 설치용 아카이브 |
| `SHA256SUMS` | 다운로드 무결성 검사 |
| `install.sh` | Mac 기본 도구만 사용하는 설치 프로그램 |
| `source-revision.txt` | 빌드한 Git 커밋 |

로컬 패키징은 다음과 같다. `--binary`는 기존 arm64 실행 파일을 사용하며 다시 빌드하지 않는다.

```sh
./scripts/release-macos.sh --version v1.0.7
./scripts/release-macos.sh --version v1.0.7 --binary /absolute/path/to/pokarak
python3 scripts/test-icon-assets.py
python3 scripts/test-distribution.py
```

## Apple 서명과 공증

현재 개발 패키지와 로컬 설치는 ad-hoc arm64 서명 경로를 검증했다. Apple Developer 계정이나 Developer ID 인증서가 없어 정식 공증을 수행하지 않는다. 브라우저로 다운로드한 앱에는 macOS 보안 경고가 표시될 수 있다. CLI 설치도 모든 기기·보안 정책에서 경고가 없음을 보장하지 않는다.

나중에 Developer ID 인증서와 `notarytool` 키체인 프로필을 준비하면 로컬 릴리스 스크립트에서 서명·공증을 켤 수 있다.

```sh
SIGNING_IDENTITY='Developer ID Application: Your Name (TEAMID)' \
NOTARY_KEYCHAIN_PROFILE='forkarak-notary' \
./scripts/release-macos.sh --version v1.0.7
```

이 경로는 hardened runtime과 timestamp로 서명하고, `notarytool submit --wait` 성공 후 ticket을 staple한 번들을 배포한다. 실제 계정이 없어 공증 경로의 Apple 서비스 연동은 검증되지 않았다. 기본 GitHub 워크플로는 인증서와 공증 자격 증명을 구성하지 않으며 ad-hoc 빌드를 게시한다. 공증에는 [Apple이 요구하는 Developer ID 서명](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)이 필요하다. 공증한 앱에도 일반적인 첫 실행 확인창은 남을 수 있다.
