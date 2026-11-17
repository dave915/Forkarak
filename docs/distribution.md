# CLI 설치와 릴리스

지원 대상은 Apple Silicon Mac, macOS 13 이상이다. 배포 파일은 네이티브 Rust/GPUI 앱이며, 설치하는 Mac에서 앱을 컴파일하지 않는다. Intel Mac과 Windows/Linux 설치는 지원하지 않는다. 최소 OS는 선언값이며 macOS 13 실기기 검증은 별도다.

## 설치와 업데이트

공개 GitHub Releases의 최신 안정 버전을 설치한다.

```sh
/bin/bash -o pipefail -c 'curl -fsSL https://github.com/dave915/pokarak/releases/latest/download/install.sh | /bin/bash'
~/.local/bin/pokarak --version
~/.local/bin/pokarak /path/to/project
```

기본 경로는 `~/Applications/Pokarak.app`과 `~/.local/bin/pokarak`이다. `sudo`나 Xcode, Rust, Homebrew, GitHub 계정이 필요하지 않다. 설치 프로그램은 앱을 자동으로 실행하거나 셸 설정을 수정하지 않는다. `~/.local/bin`을 PATH에 추가하면 `pokarak`으로 실행할 수 있다.

v0.2.0까지의 `포카락.app`은 앱을 종료한 뒤 위 설치 명령을 한 번 실행하면 `Pokarak.app`으로 이동한다. 사용자 지정 경로는 기존과 같은 `--app-dir`, `--bin-dir`을 지정한다.

앱을 종료하고 업데이트한다. 설치 때 지정한 앱·명령 경로를 다시 사용한다.

```sh
~/.local/bin/pokarak update
```

v0.2.1 이하에서 최신 버전 확인이 403으로 실패하면 `~/.local/bin/pokarak update --version v0.2.4`로 해당 조회를 건너뛸 수 있다. v0.2.2부터는 공개 릴리스 페이지의 리디렉션으로 태그를 확인한다. 패키지 또는 체크섬 다운로드 자체가 거부되면 설치 프로그램이 실패한 URL을 표시한다.

설치 스크립트를 내려받아 옵션을 지정할 수도 있다. `--version`은 버전을 고정할 때 사용하며 해당 GitHub 릴리스가 있어야 한다.

```sh
installer="$(mktemp)"
curl -fsSL https://github.com/dave915/pokarak/releases/latest/download/install.sh -o "$installer" && \
  /bin/bash "$installer" --version v0.2.4 --app-dir "$HOME/Applications" --bin-dir "$HOME/.local/bin"
rm -f "$installer"
```

업데이트는 사용자 저장소, `~/.config/pokarak/session.json`, 분석 도구와 인덱스를 수정하지 않는다. 언어 서버 설치는 기존 `scripts/setup-language-servers.sh`의 별도 명시적 작업이다.

## 다운로드와 교체

설치 프로그램은 최신 릴리스 태그를 한 번 결정한 뒤 같은 태그에서 앱과 `SHA256SUMS`를 받는다. SHA-256, `dev.pokarak.app` 식별자, 버전, arm64 실행 파일, 코드 서명을 검사한 뒤 설치한다. SHA-256과 ad-hoc 서명은 파일 손상 검출에 쓰며 Apple의 개발자 신원 인증을 뜻하지 않는다.

검증에 실패하면 설치된 앱을 교체하지 않는다. 관련 없는 앱이나 명령, 심볼릭 링크를 덮어쓰지 않는다. 앱 실행 중에는 종료 후 재시도를 안내한다. 세부 검사와 오류 경로는 `scripts/test-distribution.py`에서 임시 앱과 격리된 설치 디렉터리로 검증한다.

## 새 버전 게시

`.github/workflows/release.yml`은 `vX.Y.Z` 태그를 push하면 macOS arm64 환경에서 설치 테스트, 앱 테스트, release 빌드와 패키징을 수행한다. 성공한 산출물만 새 릴리스로 게시한다. 같은 태그의 기존 릴리스 파일은 자동으로 덮어쓰지 않는다. GitHub 문서의 [최신 릴리스 다운로드 URL](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases)을 사용하므로 설치 명령은 버전마다 바뀌지 않는다.

1. 기능 변경을 검증하고 통합한다.
2. `Cargo.toml`의 workspace 버전과 `Cargo.lock`의 로컬 패키지 버전을 갱신하고 커밋한다.
3. 해당 커밋에 같은 버전의 태그를 붙이고 원격으로 보낸다.

```sh
git tag v0.2.0
git push origin HEAD v0.2.0
```

워크플로 수동 실행은 이미 존재하는 태그를 받는다. 실패한 빌드를 고친 경우 새 버전을 사용하고, 기존 릴리스가 없는 단순 일시 오류는 같은 태그의 워크플로를 재실행한다.

로컬 산출물은 `dist/release/`에 생기며, 각 GitHub 릴리스에는 다음 파일이 있다.

| 파일 | 내용 |
| --- | --- |
| `Pokarak-macos-arm64.zip` | Finder에서 압축을 풀어 사용하는 `Pokarak.app` |
| `pokarak-macos-arm64.tar.gz` | 기존 CLI와 호환되는 설치용 아카이브 |
| `SHA256SUMS` | 다운로드 무결성 검사 |
| `install.sh` | Mac 기본 도구만 사용하는 설치 프로그램 |
| `source-revision.txt` | 빌드한 Git 커밋 |

로컬 패키징은 다음과 같다. `--binary`는 기존 arm64 실행 파일을 사용하며 다시 빌드하지 않는다.

```sh
./scripts/release-macos.sh --version v0.2.4
./scripts/release-macos.sh --version v0.2.4 --binary /absolute/path/to/pokarak
python3 scripts/test-distribution.py
```

## Apple 서명과 공증

현재 배포는 ad-hoc 서명이다. Apple Developer 계정이나 Developer ID 인증서가 없어 정식 공증을 수행하지 않는다. 브라우저로 다운로드한 앱에는 macOS 보안 경고가 표시될 수 있다. CLI 설치도 모든 기기·보안 정책에서 경고가 없음을 보장하지 않는다.

나중에 Developer ID 인증서와 `notarytool` 키체인 프로필을 준비하면 로컬 릴리스 스크립트에서 서명·공증을 켤 수 있다.

```sh
SIGNING_IDENTITY='Developer ID Application: Your Name (TEAMID)' \
NOTARY_KEYCHAIN_PROFILE='pokarak-notary' \
./scripts/release-macos.sh --version v0.2.4
```

이 경로는 hardened runtime과 timestamp로 서명하고, `notarytool submit --wait` 성공 후 ticket을 staple한 번들을 배포한다. 실제 계정이 없어 공증 경로의 Apple 서비스 연동은 검증되지 않았다. 기본 GitHub 워크플로는 인증서와 공증 자격 증명을 구성하지 않으며 ad-hoc 빌드를 게시한다. 공증에는 [Apple이 요구하는 Developer ID 서명](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)이 필요하다. 공증한 앱에도 일반적인 첫 실행 확인창은 남을 수 있다.
