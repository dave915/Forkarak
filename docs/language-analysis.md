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

The UI's existing per-worktree trust action warns that Gradle/Maven can execute project code. Its explicit `NavigatorOptions { allow_project_imports: true, ..Default::default() }` is sufficient; there is no additional approval in the navigation crate. Instantiating a Navigator still starts no server. Servers start on navigation and use an isolated worktree/instance state directory. Official Kotlin configurations assigned to both JVM languages share a process within one Navigator and never receive JDT's `-data` flag.

Kotlin project import occurs after LSP initialization. The client checks `intellij/workspaceImportState`, services messages while import is pending, and reports failed/blocked imports as errors instead of empty semantic success. The initialize/import budget is 120 seconds by default, including official Kotlin navigation; other navigation requests retain a 10-second default. Applications can override the existing timeout options. Protocol import/log messages are bounded and exposed through `server_messages()`; stderr remains separately available.

Managed Kotlin starts with `--data-sharing none`. Setup does not inject an EULA acceptance hash, a license, or account credentials. This installed bundle started and performed the verified Kotlin operations without the client supplying them. If a different release or machine requires vendor consent/licensing, the server's errors remain visible; Pokarak does not bypass those requirements.

## Empirical results and limitations

All fixtures were authored in temporary Git repositories, with source/JVM target 17. No user project was used for validation.

| Request | Actual result |
| --- | --- |
| Java call → Java definition, JDT LS | Passed |
| Java symbol → Java callers, JDT LS | Passed |
| Kotlin call → Kotlin definition, Kotlin-only Maven project | Passed |
| Kotlin symbol → Kotlin callers, Kotlin-only Maven project | Passed |
| Kotlin call → Kotlin definition, mixed Maven project | Passed |
| Kotlin call → Java definition, mixed Maven project | Passed |
| Kotlin symbol → references including a Java caller | Passed |
| Java call → Kotlin definition, Kotlin LSP alone | Returned no locations; unsupported in this tested configuration |
| Java symbol → Kotlin callers, Kotlin LSP alone | Returned no locations; unsupported in this tested configuration |
| JS/TS/TSX/JSX definitions, TS references/implementation, disk updates | Passed with managed tools and `/usr/bin:/bin` PATH |

**Full bidirectional Java/Kotlin navigation is not established.** The official extension includes Java files to observe changes in mixed projects; that alone does not promise Java-source feature providers. See the [upstream document-selector implementation](https://github.com/Kotlin/kotlin-lsp/blob/main/vscode-extension-core/src/lspClient.ts). Defaults therefore keep JDT LS for Java and JetBrains Kotlin LSP for Kotlin.

JDT reference results explicitly include `JDT LS Java scope: Kotlin callers may be missing`. Kotlin-symbol references including Java callers were verified from a Kotlin source position; this does not establish Kotlin caller coverage when starting from a Java symbol through JDT. The optional Java-source probes in the mixed test print the real returned locations and do not turn empty responses into positive support assertions.

Gradle importing has not been validated here. Kotlin Maven import actually ran Maven, installed its importer plugin, resolved dependencies, and invoked `generate-sources` in the temporary fixtures. In trusted real projects these actions can use the Maven cache and execute project plugins; this is why the UI's import warning and explicit opt-in matter. Large projects, custom source sets, archive-source navigation, and all JVM implementation requests remain outside this validation.

## Reproduce checks

Use `hc run --bg` / `hc wait` for these Cargo commands in Herdr. Keep the shared target cache; do not delete it.

```sh
export PATH=/Users/han/.cargo/bin:/usr/bin:/bin
export CARGO_TARGET_DIR=/Users/han/dev/pokarak/target
cargo test -p pokarak-navigation --features test-server
cargo test -p pokarak-navigation --test jvm -- --ignored --nocapture --test-threads=1
cargo test -p pokarak-navigation --test typescript -- --ignored --nocapture
cargo clippy -p pokarak-navigation --all-targets --features test-server -- -D warnings
```

The real-server tests install nothing. `POKARAK_TOOLS_DIR` can point tests at a custom manifest; the TS test also retains the optional `POKARAK_TS_SERVER` override for an externally managed executable. Deterministic tests cover managed manifest discovery, official JVM session sharing, import failure handling, existing framing/synchronization behavior, worktree isolation, and cleanup.
