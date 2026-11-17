# Language analysis setup and verified scope

Run `scripts/setup-language-servers.sh` explicitly to install managed tools. The app does not download tools or execute this script at startup. On this Apple Silicon Mac the default destination is `~/.local/share/pokarak/tools`.

```sh
# In Herdr, run in a background program and collect its exit status.
hc run --bg --name language-setup -- /bin/sh scripts/setup-language-servers.sh
hc wait language-setup
hc read language-setup --lines 30
```

Python 3, curl, and tar are needed for setup. The script uses official release metadata and verifies downloaded archives against published SHA-256/SHA-512 digests. Its recorded installations are reused on subsequent runs without metadata requests or downloads. `--update` explicitly resolves newer releases; `--tools-dir /absolute/path` selects a different installation destination. Only the default destination is discovered automatically by the app; custom destinations can be loaded with `installed_configs(path)` or used in user overrides.

The script writes `installed.json` (versions, source URLs, checksums, executable argv), `language-servers.example.json`, and an experimental `language-servers.kotlin-jvm.example.json` inside the tools directory. It does not write `~/.config/pokarak/language-servers.json`, shell profiles, global `JAVA_HOME`, or inspected projects. The Kotlin/JVM example is for reproducing capability probes, **not** a claim of Java-source navigation support.

### Kotlin만 설치하기

Kotlin 정의·사용처 탐색에 필요한 서버는 Homebrew로 설치할 수 있습니다. 설치한 뒤 포카락의 오류 행에서 **다시 시도**를 누르세요.

```sh
brew install JetBrains/utils/kotlin-lsp
```

별도 경로에 설치했다면 `~/.config/pokarak/language-servers.json`에서 `kotlin`의 `program`을 실행 파일의 절대 경로로 지정할 수 있습니다. [JetBrains의 Kotlin 서버 설치 안내](https://github.com/Kotlin/kotlin-lsp#install-kotlin-lsp-cli)

## Installed runtimes

These versions were installed and tested on 2026-11-16:

| Tool | Version | Use |
| --- | --- | --- |
| Eclipse Temurin JDK | 25.0.5+8 | JDT LS runtime, addressed by absolute `bin/java` |
| Eclipse JDT LS | 1.63.0 | Java navigation |
| Official JetBrains Kotlin LSP | 271.2085.0 | Kotlin and supported cross-language navigation; bundled JetBrains Runtime 25 |
| Apache Maven | 3.10.1 | Kotlin server's Maven project importer |
| Node | 22.23.5 | Absolute executable for the TS adapter |
| typescript-language-server | 5.3.0 | JS/TS/JSX/TSX LSP adapter |
| TypeScript | 6.0.3 | Compatible tsserver implementation |

The current official Kotlin bundle is from **Kotlin/kotlin-lsp / JetBrains**. It is not fwcd's `kotlin-language-server`. Source-level Java 17 remains defined by the project's build/Eclipse configuration. For Maven/Gradle imports, setup discovers an existing JDK 17 with `java_home -v 17` and includes it as a **child-only** `JAVA_HOME` assignment. It does not install a JDK 17 or alter the application's/global environment. JDT is launched directly with JDK 25 and Kotlin's native launcher uses its bundled runtime.

The TS command is an absolute managed Node executable followed by an absolute `typescript-language-server/lib/cli.mjs` and `--stdio`. The Kotlin command uses `/usr/bin/env` with an explicit managed Maven/system PATH, optional child JDK 17 assignment, and the absolute native `intellij-server` launcher. These are argv arrays, not shell commands. Both ran with a GUI-like `/usr/bin:/bin` PATH. The generated manifest contains the exact paths; none refer to `/tmp` installations.

Official metadata: [Temurin 25 releases](https://github.com/adoptium/temurin25-binaries/releases), [JDT milestone listing](https://download.eclipse.org/justj/?file=jdtls%2Fmilestones), [Kotlin LSP releases](https://github.com/Kotlin/kotlin-lsp/releases), [Node release index](https://nodejs.org/dist/index.json), [Apache Maven metadata](https://repo.maven.apache.org/maven2/org/apache/maven/apache-maven/maven-metadata.xml).

## App integration

`default_configs()` reads the installed manifest without launching anything and prefers it over PATH fallbacks. User configuration remains an override. Malformed or unavailable managed installations can be inspected with `installed_configs(path) -> Result<Vec<ServerConfig>>`; rerun setup if the installation is missing. There is no app-startup repair/download path.

The UI's existing per-worktree trust action warns that Gradle/Maven can execute project code. Its explicit `NavigatorOptions { allow_project_imports: true, ..Default::default() }` is sufficient; there is no additional approval in the navigation crate. Instantiating a Navigator still starts no server. Servers start on navigation and use an isolated worktree/instance state directory. Official Kotlin configurations assigned to both JVM languages share a process within one Navigator and the same discovered import root and never receive JDT's `-data` flag.

Kotlin project import occurs after LSP initialization. The client checks `intellij/workspaceImportState`, services messages while import is pending, and reports failed/blocked imports as errors instead of empty semantic success. The initialize/import budget is 120 seconds by default, including official Kotlin navigation; other navigation requests retain a 10-second default. Applications can override the existing timeout options. Protocol import/log messages are bounded and exposed through `server_messages()`, including the last session that failed during navigation; stderr remains separately available. Initialization failures retain their raw diagnostics in the returned error chain.

Managed Kotlin starts with `--data-sharing none`. Setup does not inject an EULA acceptance hash, a license, or account credentials. This installed bundle started and performed the verified Kotlin operations without the client supplying them. If a different release or machine requires vendor consent/licensing, the server's errors remain visible; Pokarak does not bypass those requirements.

## Build root selection and isolation

For Kotlin and JDT, the selected source's ancestor directories are checked inside the canonical worktree only. Discovery reads metadata; it never runs a build or installs/scaffolds project configuration.

- Prefer the outermost enclosing `settings.gradle[.kts]` or Maven aggregator whose `<modules>` entry contains the selected source. Maven XML is parsed, so commented-out modules do not count. Module paths must resolve beneath the aggregator. Dynamic/property-expanded Maven module paths are not evaluated.
- Otherwise use the nearest `pom.xml`, `build.gradle[.kts]`, or settings file. JDT also accepts a directory containing both `.project` and `.classpath`. A nested Git boundary ends the ancestor walk. Build-file symlinks outside the worktree are ignored.
- With no supported configuration, return `noBuildSystemFound` with a concise request to check Maven/Gradle metadata. No server is launched for this failure, and no text/regex result is represented as semantic navigation.
- The selected import root supplies process cwd, LSP `rootUri`/`rootPath`, and workspace folders. For a monorepo it can be `worktree/backend` while `Navigator::root()` remains the canonical **worktree**. The public `{root}` argv token still means the canonical worktree; `{workspace}` remains the isolated temporary server state directory. Result filtering, stale-response guards, and instance isolation still use the worktree.
- Independent nested builds have distinct clients. An external dependency source returned by a client remembers that client/configuration/import root, including when its language differs. If multiple clients return the same dependency path, the most recent result establishes its navigation origin. Another checkout of the same Git repository is still rejected.

Kotlin cannot start without `allow_project_imports=true` (or the existing explicit `allowProjectImports` configuration). JDT's Maven/Gradle import settings remain disabled until that option is true, and automatic builds remain disabled. A supported build marker alone never grants trust. Gradle settings discovery is covered deterministically; real Gradle importing has not been verified.

## Error and Output presentation

The existing `self.error: Option<String>` sink is unchanged, so Git-operation errors use the same presentation. A fixed 32px row near the bottom status controls shows a single human-readable summary, **Details**, dismiss, and retry for a failed analysis request when its original worktree/file is still selected. `noBuildSystemFound` says in Korean that Maven/Gradle project metadata is missing; JDK guidance is shown only for concrete runtime evidence such as `UnsupportedClassVersionError`, an invalid `JAVA_HOME`, or a missing Java runtime. Generic import failures no longer carry a Java 17 warning.

**Details** opens the bottom **Output** panel. Raw errors, protocol/import messages, and stderr stay there, with virtualized vertical/horizontal scrolling and **모두 복사**. Output and search/usages occupy the same bottom space, never two stacked result panels. Each is capped at 210px and 30% of the window; Output starts collapsed and can be closed. File and diff viewing remain available during import or failure. The retained Output tail is bounded to 128 KiB, survives dismissing an error, and is cleared when changing worktrees/projects. Stale async results cannot append another worktree's logs. Native visual verification is intentionally left to the coordinating task; this change does not launch or stop the user's app.

## Authored UI QA fixture repair

`scripts/repair-ui-analysis-fixture.py` is a manual, narrowly guarded QA repair. It reads `/tmp/pokarak-ui-fixture-path`, requires the authored `pokarak-ui-check-*/sample-fullstack` layout and sibling `api-worktree`, verifies both README files contain `검증용`, verifies Git worktree roots and the absence of `origin`, and refuses to overwrite any existing Maven/Gradle build files. It adds only a root `pom.xml`, using `backend/src/main/java`, `backend/src/main/kotlin`, Java/JVM target 17, Kotlin Maven plugin 2.2.21 and Maven compiler plugin 3.14.1. It does not modify source files or commit/discard fixture changes. This script is never called by the app.

On 2026-11-17 both current authored QA worktrees were repaired. The equivalent authored source-layout fixture and a nested Maven aggregator fixture were then checked with the real official Kotlin LSP. This does not introduce automatic scaffolding for real projects.

## Empirical results and limitations

All fixtures were authored in temporary Git repositories, with source/JVM target 17. No user project was used for validation.

| Request | Actual result |
| --- | --- |
| Java call → Java definition, JDT LS | Passed |
| Java symbol → Java callers, JDT LS | Passed |
| Java definitions/references, nested Maven backend, JDT LS | Passed (2026-11-17) |
| Kotlin call → Kotlin definition, Kotlin-only Maven project | Passed |
| Kotlin symbol → Kotlin callers, Kotlin-only Maven project | Passed |
| Kotlin call → Kotlin definition, mixed Maven project | Passed |
| Kotlin call → Java definition, mixed Maven project | Passed |
| Kotlin symbol → references including a Java caller | Passed |
| Kotlin→Java definition and Kotlin references, nested Maven aggregator | Passed (2026-11-17) |
| Kotlin→Java definition and Kotlin references, QA backend source layout | Passed (2026-11-17) |
| Java call → Kotlin definition, Kotlin LSP alone | Returned no locations; unsupported in this tested configuration |
| Java symbol → Kotlin callers, Kotlin LSP alone | Returned no locations; unsupported in this tested configuration |
| JS/TS/TSX/JSX definitions, TS references/implementation, disk updates | Passed with managed tools and `/usr/bin:/bin` PATH |

**Full bidirectional Java/Kotlin navigation is not established.** The official extension includes Java files to observe changes in mixed projects; that alone does not promise Java-source feature providers. See the [upstream document-selector implementation](https://github.com/Kotlin/kotlin-lsp/blob/main/vscode-extension-core/src/lspClient.ts). Defaults therefore keep JDT LS for Java and JetBrains Kotlin LSP for Kotlin.

JDT reference results explicitly include `JDT LS Java scope: Kotlin callers may be missing`. Kotlin-symbol references including Java callers were verified from a Kotlin source position; this does not establish Kotlin caller coverage when starting from a Java symbol through JDT. The optional Java-source probes in the mixed test print the real returned locations and do not turn empty responses into positive support assertions.

Gradle importing has not been validated here. Kotlin Maven import actually ran Maven, installed its importer plugin, resolved dependencies, and invoked `generate-sources` in the temporary fixtures. In trusted real projects these actions can use the Maven cache and execute project plugins; this is why the UI's import warning and explicit opt-in matter. Large projects, source sets beyond the tested backend layout, archive-source navigation, and all JVM implementation requests remain outside this validation.

## Reproduce checks

Use `hc run --bg` / `hc wait` for these Cargo commands in Herdr. Keep the shared target cache; do not delete it.

```sh
export PATH=/Users/han/.cargo/bin:/usr/bin:/bin
export CARGO_TARGET_DIR=/Users/han/dev/pokarak/target
cargo test -p pokarak-navigation --features test-server
cargo test -p pokarak-navigation --test jvm -- --ignored --nocapture --test-threads=1
cargo test -p pokarak-navigation --test typescript -- --ignored --nocapture
cargo test -p pokarak analysis_feedback
cargo check -p pokarak
cargo clippy -p pokarak-navigation --all-targets --features test-server -- -D warnings
```

The real-server tests install nothing. `POKARAK_TOOLS_DIR` can point tests at a custom manifest; the TS test also retains the optional `POKARAK_TS_SERVER` override for an externally managed executable. Deterministic tests cover managed manifest discovery, official JVM session sharing, import failure handling, existing framing/synchronization behavior, worktree isolation, nested import-root selection, unchanged `{root}` expansion, dependency-client affinity, no-build-config failures before startup, JDT import gating, and cleanup. App tests cover bounded human-readable summaries, conditional runtime guidance, raw-output retention after dismiss, and worktree isolation.
