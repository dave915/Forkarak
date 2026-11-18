# Language analysis and managed tools

이 문서는 현재 개발 버전의 언어 지원과 관리 도구를 설명합니다. [설치와 소스 빌드](distribution.md)도 참고하세요.

## Start in the app

File viewing, syntax highlighting and text search work immediately, without installation, indexing or language-server startup. **코드 지원…** provides completion, diagnostics, file symbols, function help, hover/type information and code navigation when the selected server supports them. 작업 화면 선택과 코드 지원은 별개입니다. 작업 파일의 [바로 편집](../README.md#편집과-탐색)은 언어 분석 허용을 뜻하지 않습니다.

설정의 **언어 분석·포맷 → 언어 서버 선택·설치…**에서 사용할 언어를 켜고 서버를 설치하세요. 1.0.3에서는 같은 버튼이 **설치·업데이트 관리…**로 표시됩니다. 도구가 준비되면 대상 워크트리에서 분석을 허용하세요. 기본 서버는 관리 화면에서 구성되며, 직접 실행 경로와 인수를 지정할 때만 **사용자 지정 서버·포맷터…**를 펼치면 됩니다. The manager distinguishes bundled tools, first-use downloads and unsupported/prerequisite-limited entries. Use **설치**, **업데이트 다운로드**, **취소** or **다시 시도** explicitly; version, source, checksum and license/notice details remain inspectable. Failed or cancelled updates retain the previous active installation. A pending code action can be resumed with **원래 요청 계속** only after the original file, source and trust are rechecked.

분석을 켠 코드 편집기에서는 다음 동작을 사용할 수 있습니다.

- **자동완성**은 입력 위치에 후보와 설명을 표시합니다. **⌃Space**로 직접 열고 ↑·↓로 선택한 뒤 Return으로 초안에 적용합니다. 설정에서 **입력 중 자동완성**을 끌 수 있습니다.
- **⌘I**로 함수 시그니처와 현재 인자를, **⌥F1**으로 커서에 있는 심볼의 타입과 설명을 확인합니다. **입력 중 함수 도움말**을 켜면 여는 괄호와 쉼표에서도 도움말이 열립니다.
- **F12**는 정의가 하나면 해당 코드로 이동합니다. **⌥⇧F12**는 사용처를 파일별 코드 문맥과 함께 편집기 탭에 표시합니다. 결과 상단이나 탐색 설정에서 기존 팝오버·파일 영역·하단 배치도 선택할 수 있습니다.

- Default macOS packages include **Eclipse JDT LS 1.63.0 + Temurin 25.0.5+8** for Java. No Homebrew or user-installed Java is needed to run this server.
- **Kotlin** uses the official JetBrains 271.2085.0 first-use download, with its runtime and managed Maven. Kotlin is partially proprietary; the repository's Apache license does not establish a redistribution grant for the complete binary. It is not bundled or replaced with a paid unified server.
- The **18 managed packages in total (including Java and Kotlin)** cover JS/TS, Rust, Python, Go, Groovy/Gradle, HTML, CSS/SCSS/Less, Vue, Svelte, JSON/JSONC, YAML, TOML, XML, SQL, Markdown, Shell, Dockerfile and Makefile. Their runtimes are app-managed; no terminal, npm, Python or Homebrew setup is required for those server runtimes.

Installation and analysis trust are separate. Installing tools does not start a server, build or import a project. Explicitly allow analysis/import for the worktree when requested. Maven/Gradle imports can execute project plugins; Rust, Go, Groovy, Svelte and SQL also require import permission before startup. The app rejects stale results after a worktree or source change. Historical snapshots never analyze against current working files.

App-owned runtime independence from PATH does not supply every project's target SDK/JDK, libraries or build configuration. Keep the project's target JDK distinct from the JDK used to launch JDT/Kotlin. External servers run as separate trusted processes; they are not OS-sandboxed.

## Verified language scope

The authoritative [tested support matrix](../crates/navigation/resources/language-support.md) records exact versions, real temporary-repository probes, negotiated features and limitations. Server installation is not proof that every LSP method works.

| Family | Verified examples and limits |
| --- | --- |
| Java / Kotlin | Dirty-buffer completion, hover, signature help, symbols and formatting were tested, alongside navigation. JDT Java references can omit Kotlin callers; full bidirectional Java/Kotlin navigation is not established. Archive URI extraction is unavailable. |
| JS / TS / JSX / TSX | Definitions/references/implementation and dirty-buffer editor features tested. Compatible TypeScript 5.9.3 + typescript-language-server 6.1.0 are managed; TypeScript 7's native compiler is not a tsserver replacement. |
| Rust / Python / Go / Groovy | Real navigation or symbol probes vary by server. Pyright and the selected Groovy server provide no formatter. Rust build scripts/proc macros/check-on-save are disabled. |
| Web / structured files | Real completion or structure probes, as listed in the matrix. **Vue is pinned to standalone 2.2.12**; the modern Vue 3 TypeScript bridge is absent. Schema-aware features need appropriate schemas. |
| SQL / Markdown / Shell / build files | SQL database features need explicit connections; no query execution action is exposed. ShellCheck/shfmt are optional custom tools. Docker and make are never invoked for viewing or these static probes. |
| Generic Properties / .env | Viewing and text search only. Spring Properties needs the STS/JDT classpath/data bridge, which is not implemented. A bundled JVM alone does not provide it. |

Only real semantic responses are labeled semantic. Text candidates remain labeled as text matches. Unsupported methods, source changes and import failures are surfaced explicitly. Unversioned push diagnostics are unavailable unless a supported snapshot-bound pull path exists; diagnostics and formatter availability depend on the server.

## Custom servers and legacy setup

Custom settings remain authoritative: installing managed tools never replaces an explicit program/argv. The advanced configuration is `~/.config/pokarak/language-servers.json`, outside the inspected repository. For example:

```json
{
  "servers": [
    {
      "language": "typescript",
      "program": "/absolute/path/to/typescript-language-server",
      "args": ["--stdio"]
    },
    {
      "language": "kotlin",
      "program": "/absolute/path/to/kotlin-server/bin/intellij-server",
      "args": ["--stdio", "--system-path", "{workspace}/system", "--data-sharing", "none"]
    }
  ],
  "allowProjectImports": false
}
```

Programs are absolute executables or names on PATH. Arguments are individual argv values, with no shell expansion of `~`, `$VARIABLE` or globs. `{root}` means the canonical worktree; `{workspace}` is an isolated server-state directory outside it. Java `-data` is isolated even with a custom launcher. See the [configuration schema and direct Java argv example](../crates/navigation/README.md#server-configuration) for supported IDs, aliases, overrides and discovery order. A broken custom path reports an error rather than silently selecting another installation.

The older explicit `scripts/setup-language-servers.sh` remains available for developer-managed installations; it is not the native manager's prerequisite and is never executed at app startup.

```sh
./scripts/setup-language-servers.sh
./scripts/setup-language-servers.sh --update
./scripts/setup-language-servers.sh --tools-dir /absolute/path/to/tools
```

This legacy script requires Python 3, curl and tar on the setup machine. Its default destination is `~/.local/share/pokarak/tools`. It writes `installed.json`, `language-servers.example.json` and an experimental Kotlin/JVM example there, without editing shell profiles, global JAVA_HOME or project files. Custom destinations require an explicit configuration; only the default legacy location is discovered automatically. The experimental Java-through-Kotlin example is a capability probe, not a support claim. Use the manager's current matrix for production package pins; the legacy script can resolve different versions.

Existing external installations are still supported, including `brew install jdtls` and `brew install JetBrains/utils/kotlin-lsp` as optional advanced alternatives. Discovery checks files without installing or starting anything. An explicit absolute custom program takes precedence over managed tools and discovery. See [official Kotlin installation guidance](https://github.com/Kotlin/kotlin-lsp#installation).

## Build root selection and isolation

For Kotlin and JDT, the selected source's ancestor directories are checked inside the canonical worktree only. Discovery reads metadata; it never runs a build or installs/scaffolds project configuration.

- Prefer the outermost enclosing `settings.gradle[.kts]` or Maven aggregator whose `<modules>` entry contains the selected source. Maven XML is parsed, so commented-out modules do not count. Module paths must resolve beneath the aggregator. Dynamic/property-expanded Maven module paths are not evaluated.
- Otherwise use the nearest `pom.xml`, `build.gradle[.kts]`, or settings file. JDT also accepts a directory containing both `.project` and `.classpath`. A nested Git boundary ends the ancestor walk. Build-file symlinks outside the worktree are ignored.
- With no supported configuration, return `noBuildSystemFound` with a concise request to check Maven/Gradle metadata. No server is launched for this failure, and no text/regex result is represented as semantic navigation.
- The selected import root supplies process cwd, LSP `rootUri`/`rootPath`, and workspace folders. For a monorepo it can be `worktree/backend` while `Navigator::root()` remains the canonical **worktree**. The public `{root}` argv token still means the canonical worktree; `{workspace}` remains the isolated temporary server state directory. Result filtering, stale-response guards, and instance isolation still use the worktree.
- Independent nested builds have distinct clients. An external dependency source returned by a client remembers that client/configuration/import root, including when its language differs. If multiple clients return the same dependency path, the most recent result establishes its navigation origin. Another checkout of the same Git repository is still rejected.

Kotlin cannot start without `allow_project_imports=true` (or the existing explicit `allowProjectImports` configuration). JDT's Maven/Gradle import settings remain disabled until that option is true, and automatic builds remain disabled. A supported build marker alone never grants trust. The Gradle fixture below verifies discovery of an enclosing settings project and cross-module navigation after an authorized import.

## Failures and output

A failed request shows a concise summary, **Details** and, when its original worktree/file is still current, retry. Missing Maven/Gradle metadata is distinct from a missing server runtime or invalid JAVA_HOME. **Details** opens the bounded **Output** panel with raw errors, import messages and stderr, scrolling and **모두 복사**. Search/usages and Output share the bottom area. Viewing remains available during import or failure; another worktree's stale output cannot overwrite the current one.

## JVM checks and remaining limits

Authored Java 17, Kotlin-only, mixed Maven, nested Maven aggregator and Gradle multi-module repositories were used. Java→Java definitions/references, Kotlin→Kotlin definitions/references and Kotlin→Java definitions passed. References from a Kotlin symbol included a Java caller. Starting at a Java symbol through Kotlin alone returned no locations in the tested setup; JDT therefore remains Java's default. This does not establish complete Kotlin caller coverage from Java.

The Gradle probe used Gradle Wrapper 9.3.0, Kotlin JVM plugin 2.3.21 and Kotlin LSP 271.2085.0. After explicit trust, a call navigated across two modules into a custom source folder and references returned to the caller. First import may download Gradle distributions/plugins and dependencies. Maven probes ran importer plugins and generate-sources only in authored temporary projects; real trusted imports can likewise execute project tooling. Large-project performance and untested source-set layouts remain unverified.

## Reproduce checks

In Herdr, run long commands with `hc run --bg` and collect them with `hc wait`. Keep the shared Cargo cache. The deterministic suite uses authored temporary repositories; real-server checks are opt-in and install nothing.

```sh
cargo test --workspace --features pokarak-navigation/test-server
cargo test -p pokarak-navigation --test jvm -- --ignored --nocapture --test-threads=1
cargo test -p pokarak-navigation --test typescript -- --ignored --nocapture
cargo test -p pokarak-navigation --test managed_real -- --ignored --nocapture --test-threads=1
```

For managed probes, set `FORKARAK_MANAGED_TOOLS_DIR` to an explicitly prepared tool directory. `FORKARAK_BUNDLED_RESOURCES` points at actual packaged Contents/Resources for the relocation probe. Legacy tests retain `POKARAK_TOOLS_DIR` and `POKARAK_TS_SERVER` overrides. See [navigation tests and contracts](../crates/navigation/README.md) and [Java packaging notices](../scripts/language-resources-NOTICE.md); backend checks are separate from native UI acceptance.
