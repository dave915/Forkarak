# Forkarak navigation

A synchronous, read-only stdio LSP client. The app must obtain project trust **before creating a Navigator**, run all navigation, refresh, and shutdown calls on a background thread, and associate every pending UI operation with its worktree/generation. Switching worktrees must discard old UI responses. A Navigator never changes roots or shares a process/document cache with another instance.

The scaffold contracts are unchanged:

```rust,ignore
let mut navigator = Navigator::new(worktree_root, default_configs());
let result = navigator.navigate(&file, line0, column_utf16, NavigationKind::Definition)?;
let status = navigator.status();
navigator.shutdown();
```

`References` includes declarations; `Implementation` uses `targetSelectionRange` for LocationLink results. Locations use absolute filesystem paths, zero-based lines, and UTF-16 columns. A completed empty LSP response is semantic with no matches. Missing servers, unsupported methods, crashes, timeouts, malformed responses, and source changes during a request return errors, never fabricated semantic results. There is no text-search fallback.

## Cancellation on project close or app quit

Obtain a handle before putting the Navigator behind the UI's mutex:

```rust,ignore
let navigator = Navigator::with_options(root, default_configs(), options);
let cancel = navigator.cancellation_handle(); // Clone + Send + Sync
let shared = std::sync::Arc::new(std::sync::Mutex::new(navigator));
// A background worker may now hold shared.lock() throughout navigate().
// On close/quit, use the saved handle directly, without locking shared:
cancel.cancel()?;
```

`Navigator::cancellation_handle() -> CancellationHandle` is additive. The handle exposes `cancel(&self) -> anyhow::Result<()>` and `is_cancelled(&self) -> bool`. Retain one per project/worktree and invoke all relevant handles before app exit. `cancel()` permanently cancels that Navigator, synchronously signals its owned server process groups on Unix, and schedules child reaping independently. It never takes the application's Navigator mutex, waits for an LSP response, or joins I/O threads. Only short process-lifecycle locks (including OS process creation) are shared. Check its result for OS signaling errors.

In-flight reads and writes check cancellation at most every 20 ms while waiting on channels, so they do not wait out the usual request timeout, including when a detached process retains a pipe. The worker returns a cancellation error; subsequent navigation/refresh cannot start another server. Cancellation before the first request prevents startup. To resume, construct a new Navigator with a new handle. Dropping a handle alone does not cancel; repeated/concurrent cancellation and handles retained after Navigator destruction are safe. Normal `shutdown()` still attempts graceful LSP shutdown and remains source-compatible.

Process ownership, group signaling, and reaping share a private lifecycle guard. Unix status checks use `waitid(WNOWAIT)` to retain the group leader's PID until signaling is finished, and signaling is revoked before reaping. A detected external reaper revokes ownership instead of signaling a potentially reused PID. Hosts should leave these child processes to this crate rather than installing a competing global SIGCHLD reaper. Process groups deliberately escaped by descendants and Windows descendant cleanup remain outside the Unix group guarantee.

## Server configuration

`default_configs()` first prepares PATH fallbacks, then prefers each available server in the persistent manifest at `~/.local/share/pokarak/tools/installed.json` without executing anything. Run `scripts/setup-language-servers.sh` explicitly to create it. See [language analysis setup and verified scope](../../docs/language-analysis.md). Without managed tools, the fallbacks are:

| Family | File extensions | Default executable and argv |
| --- | --- | --- |
| TypeScript / JavaScript | ts, mts, cts, tsx, js, mjs, cjs, jsx | `typescript-language-server`, `['--stdio']` |
| Java | java | `jdtls`, `['-data', '{workspace}', '-configuration', '{workspace}/configuration']` |
| Kotlin | kt, kts | `kotlin-lsp`, `['--stdio', '--system-path', '{workspace}/system', '--data-sharing', 'none']` |

An unavailable executable remains in the configuration so navigation can explain what is missing. Discovery does not search project node_modules or run npx. Executables are launched with `Command::new(program).args(args)`, with no shell interpolation. Programs must be absolute paths or bare names on PATH; `~`, `$VARIABLE`, wildcards, and relative executable paths are not expanded. On macOS, Finder can supply a smaller PATH than a terminal. For the official JetBrains Kotlin server, Forkarak also checks the native standalone launcher `intellij-server` and legacy `kotlin-lsp.sh`. Fallback locations include `$KOTLIN_LSP_DIR` (the unpacked distribution), `~/.local/bin`, `~/.local/share/kotlin-lsp`, and the Homebrew prefixes `/opt/homebrew`, `/usr/local`, and `$HOMEBREW_PREFIX`. Discovery only checks files. The [upstream installation instructions](https://github.com/Kotlin/kotlin-lsp#installation) include `brew install JetBrains/utils/kotlin-lsp`. An explicit absolute `program` always takes precedence. A missing Kotlin executable is reported before Gradle or Maven root discovery.

For Java's bare `jdtls` launcher, discovery first checks absolute PATH entries, then absolute `$JDTLS_HOME` / `$JDTLS_DIR` distributions (`bin/jdtls` or `jdtls`), `~/.local/bin/jdtls`, and `~/.local/share/jdtls/{bin/jdtls,jdtls}`. It also checks `bin/jdtls`, `opt/jdtls/bin/jdtls`, and `opt/jdtls/libexec/bin/jdtls` under the same Homebrew prefixes. Relative environment paths are ignored. Explicit custom programs, absolute paths, managed direct-JVM configurations, and their argv retain precedence; a missing custom executable is never replaced by another installation. Discovery only inspects executable files and never runs a shell, installer, or server. A missing Java executable is reported in Korean before Maven/Gradle discovery, with `brew install jdtls`, the explicit setup script, and custom configuration instructions. Construction remains idle; the caller must obtain trust before creating a Navigator, and only navigation starts a server.

`Navigator::new` loads `~/.config/pokarak/language-servers.json` and merges its `servers` into the supplied configurations by normalized language ID. Exact IDs override the family entry. Malformed configuration makes `status()` and navigation report a setup error; it is never silently ignored. Missing files are fine. The default configuration location intentionally uses HOME, not the inspected repository.

Schema (all fields shown; `servers` and `allowProjectImports` are optional):

```json
{
  "servers": [
    {
      "language": "typescript",
      "program": "/absolute/path/to/typescript-language-server",
      "args": ["--stdio"]
    },
    {
      "language": "java",
      "program": "/absolute/path/to/jdk/bin/java",
      "args": [
        "-Declipse.application=org.eclipse.jdt.ls.core.id1",
        "-Dosgi.bundles.defaultStartLevel=4",
        "-Declipse.product=org.eclipse.jdt.ls.core.product",
        "--add-modules=ALL-SYSTEM",
        "--add-opens", "java.base/java.util=ALL-UNNAMED",
        "--add-opens", "java.base/java.lang=ALL-UNNAMED",
        "-jar", "/absolute/path/to/jdtls/plugins/org.eclipse.equinox.launcher_VERSION.jar",
        "-configuration", "/absolute/path/to/jdtls/config_mac",
        "-data", "{workspace}"
      ]
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

Each server requires exactly `language: string`, `program: string`, and `args: string[]`. Unknown fields and languages are errors. Supported IDs are `typescript`, `typescriptreact`, `javascript`, `javascriptreact`, `java`, and `kotlin`; aliases `ts`, `tsx`, `js`, `jsx`, `kt`, `kts`, and `js/ts` are accepted. A `typescript` entry covers the entire JS/TS family. Identical family/program/argv configurations share a server only **within** that Navigator. Official JetBrains configurations for Java and Kotlin also share a session when their program/argv are identical; they do not receive JDT flags.

Literal `{root}` and `{workspace}` tokens in argv are replaced with the canonical worktree path and an instance-specific temporary state directory. They remain single argv values, including paths containing spaces. Java `-data` is always set to an isolated directory (supplied `-data` paths are replaced); it is added if absent. State/log directories are outside the repository and removed after process cleanup. The default JDT wrapper also receives its own `-configuration` directory. With a custom Java launcher, supply the appropriate installed platform configuration yourself. Avoid launcher flags that write project outputs.

Java and Kotlin server runtimes are distinct from a project's language level. A Java 17 project does not mean a current JDT server can run on Java 17. The [JDT upstream requirements](https://github.com/eclipse-jdtls/eclipse.jdt.ls#requirements) currently specify Java 25 for the latest server; check the requirement of the version you installed. An explicit JDK executable in the configuration avoids changing the project's JDK. Startup errors include exit status, a bounded stderr tail, and runtime guidance. This crate never installs or upgrades a JDK. Java 17, Kotlin-only, and mixed Maven fixtures were validated; the exact supported directions and remaining limits are documented in [language-analysis.md](../../docs/language-analysis.md).

Kotlin's default entry now selects **official JetBrains Kotlin LSP**. The managed installation supplies its native launcher and an isolated Maven tool, with telemetry disabled. It is not the fwcd server. The UI's existing trust action passes `NavigatorOptions::allow_project_imports = true` after warning about Gradle/Maven execution; that is sufficient, with no additional hidden approval. The JSON `allowProjectImports` option remains supported. This opt-in also enables JDT Maven/Gradle imports. Kotlin import failures are returned as errors, and import completion is bounded before semantic results are delivered.

Without that opt-in, JDT gets disabled Maven/Gradle imports, disabled automatic build-configuration updates, and disabled autobuild. The TS server receives `disableAutomaticTypingAcquisition: true` and `tsserver.useSyntaxServer: "never"`; the latter prevents early definition requests from being answered by the adapter's partial syntax-only project. Its logs are directed outside the repository. Forkarak never sends build/install or arbitrary execute-command requests (the editor diagnostics adapter permits only two fixed read-only TypeScript requests), rejects server-initiated workspace edits and unsupported commands, and never executes project setup commands. External language servers are trusted executables, not sandboxed processes; a custom server is responsible for respecting these settings.

## Lifecycle and boundaries

- Construction only resolves the root and configuration. A server starts lazily on the first navigation for its configuration. No indexing is needed to view a file.
- Roots resolve symlinks and ascend to the nearest `.git` directory **or file**, preserving linked-worktree boundaries. For non-Git directories the supplied canonical directory remains the root.
- Requests use one `rootUri`/workspace folder and a root-specific process cwd. Relative source paths resolve against that root. Arbitrary external request paths are rejected; dependency sources returned by this Navigator can subsequently be navigated.
- Dependency locations outside the root are accepted. Results and requests in another worktree sharing the same Git common directory are rejected, including symlink aliases. Non-file URIs such as `jdt:`/`jar:` are skipped with an explicit result-status explanation; archive source extraction is not implemented.
- URI encoding handles spaces, percent signs, hashes, Unicode, and supplementary characters. Input columns are validated as UTF-16 boundaries. Only UTF-16 position encoding is negotiated.
- `didOpen` sends disk text once. Before each request, all previously opened documents are reread and changed text gets a versioned `didChange`, using full text or a whole-document incremental range according to server capabilities. Deleted documents get `didClose`. `refresh()` is an additive API for the app's external-change notification path; there is no autonomous filesystem watcher. The server remains responsible for unopened files and project/configuration-file watching. Restart analysis if those project inputs change without a server watcher.
- Disk changes while a request is outstanding invalidate its result. This does not resolve historical diffs against a different worktree; callers must request live source positions only.
- Initialize/import defaults to 120 seconds (also used for official Kotlin navigation), other navigation to 10 seconds, and graceful shutdown to 2 seconds per server. `Navigator::with_options` accepts `NavigatorOptions` to change these bounds and optionally disable user overrides with `config_file: None`.
- Framed JSON-RPC handles fragmented reads, interleaved notifications, unrelated IDs, and server requests for configuration, workspace folders, progress, and message choices. Unsupported client methods receive `-32601`; workspace edits receive `applied: false`. Dynamic registration and custom server commands are not supported.
- Header/body limits are 8 KiB/16 MiB, source files 8 MiB, and stderr capture retains the last 16 KiB per process. Bounded writer acknowledgments prevent a server which stops consuming stdin from blocking navigation indefinitely. Timeout/error sessions are killed and discarded; a later explicit request can start afresh.
- Explicit `shutdown()` and Drop attempt `shutdown`/`exit`, then kill and reap the direct child. On Unix, a dedicated process group also cleans up tsserver and other descendants. Detached process groups are outside this guarantee. Worker-thread joins are bounded. Windows descendant cleanup is not implemented (macOS is the target).
- `root()`, `refresh()`, `stderr()`, `server_messages()`, `installed_configs()`, `path_to_uri()`, `uri_to_path()`, `discover_program()`, and `with_options()` are additive APIs. `status()` checks child exit state even between requests.

## Verification

Run from the repository with Cargo on PATH. In Herdr use `hc run --bg` and `hc wait` as required by the project instructions.

```sh
PATH=/Users/han/.cargo/bin:$PATH cargo test -p pokarak-navigation --features test-server
PATH=/Users/han/.cargo/bin:$PATH cargo clippy -p pokarak-navigation --all-targets --features test-server -- -D warnings
```

The feature builds a deterministic Rust mock stdio peer used by integration tests. It is not part of a normal application build. Tests create isolated temporary Git repositories; no user repository is used as a test fixture. Coverage includes framing, UTF-16/URI handling, lifecycle, full/incremental disk synchronization, Location/LocationLink, references, server requests/errors/exits/timeouts, configuration, dependency sources, actual linked worktrees, and shutdown/descendant cleanup.

The ignored real-server tests install nothing. Explicitly run `scripts/setup-language-servers.sh` once, then use the managed manifest:

```sh
PATH=/Users/han/.cargo/bin:/usr/bin:/bin cargo test -p pokarak-navigation --test typescript -- --ignored --nocapture
PATH=/Users/han/.cargo/bin:/usr/bin:/bin cargo test -p pokarak-navigation --test jvm -- --ignored --nocapture --test-threads=1
```

The TS test checks real definition/references/implementation, TSX/JSX imports, UTF-16 positions, disk updates, and separate roots. The JVM suite checks real JDT Java 17 navigation and official Kotlin-only/mixed Maven navigation. Java-origin queries against the Kotlin-only bundle are capability probes and returned no locations; full bidirectional Java/Kotlin support is not claimed. JDT reference result status warns that Kotlin callers may be missing. See [the complete validation matrix](../../docs/language-analysis.md).

Protocol reference: [Language Server Protocol 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/). TypeScript initialization settings: [typescript-language-server configuration](https://github.com/typescript-language-server/typescript-language-server/blob/master/docs/configuration.md).

## Explicit analysis of editor drafts

The existing `navigate` API stays disk-backed. The additive editor API accepts an
immutable `DocumentSnapshot::new(root, path, text, version)` with the exact displayed
**working source**, a canonical worktree identity and a nonnegative LSP integer
version. Versions increase per file. Never pass a historical preview. All calls
still run on a background thread, after the existing project trust/import gate.
Construction, retaining drafts, and viewing files start no process.

```rust,ignore
let snapshot = DocumentSnapshot::new(root, path, draft, document_version)?;
let pending = snapshot.clone(); // retain outside the Navigator mutex
let definitions = navigator.navigate_buffer(&snapshot, line0, utf16, NavigationKind::Definition)?;
let completions = navigator.complete_buffer(&snapshot, line0, utf16)?;
let diagnostics = navigator.diagnostics(&snapshot)?;
let symbols = navigator.document_symbols(&snapshot)?;
let hover = navigator.hover_buffer(&snapshot, line0, utf16)?;
let signature = navigator.signature_help_buffer(&snapshot, line0, utf16)?;
// On source edit, file/worktree switch, or replacing this view:
pending.invalidate();
```

Each `BufferResult<T>` contains `snapshot` and `value`, with `is_current()`.
Invalidation is thread-safe and interrupts outstanding target-document I/O,
including initialization, without acquiring the Navigator mutex. Results also
reject changed source contexts, regressing versions, same-version/different-text
reuse, changed symlink targets and another worktree. Retain the pending clone and
invalidate it whenever the displayed document identity changes, including while a
request holds the mutex. The UI must additionally check its root/file/request
identity before applying or displaying results. `cancellation_handle()` remains
the permanent whole-Navigator stop for trust revocation, project close and quit.

`retain_buffer(&snapshot)` registers another dirty source without server startup.
The next explicit analysis request synchronizes all retained drafts for its server
session; failed sessions restore them before subsequent queries. Disk refresh
preserves these buffers. `release_buffer(path)` explicitly returns a file to disk
synchronization; legacy `navigate` refuses to replace a retained draft implicitly.
An invalidated view's last registered text stays retained until replaced or
released; invalidation alone never discards a draft. Snapshot versions identify UI
sources, while internal wire versions remain monotonic when mixed with disk calls.
Definitions and references in dirty files use their registered source for previews.
No semantic request falls back to text search.

Completion includes insertion text, optional range edits, additional edits, detail,
documentation and explicitly tagged insertion format (1 plain text, 2 snippet).
Snippet support is not advertised, but unexpected snippets retain their tag so a
consumer can reject or explicitly handle them. Keep the returned item intact for
`resolve_completion_buffer(&snapshot, &item)`: opaque server data and the source
identity are retained internally. Resolution requires server support and rejects
candidates from another file/root/version. Commands embedded in candidates are
never executed. Hover handles Markdown, plain text and legacy marked strings.
Signature help includes parameter labels or validated UTF-16 offsets, active
signature/parameter, and documentation. Unsupported capabilities are errors; null
hover/signature results are `None`. Documentation remains display-only.

Diagnostics prefer `textDocument/diagnostic`. TypeScript adapters advertising
`typescript.tsserverRequest` use only two hardcoded synchronous read-only commands:
`syntacticDiagnosticsSync` and `semanticDiagnosticsSync`, through
`workspace/executeCommand`, after the exact buffer is synchronized. Their one-based
UTF-16 locations are converted and validated. There is no public arbitrary
execute-command API. This narrow route avoids attaching unversioned asynchronous
TS reports to a newer draft; see the [adapter's command protocol](https://github.com/typescript-language-server/typescript-language-server#send-tsserver-command).
Other push-only servers must include a matching document version; unversioned
reports are an explicit error. Import/build/installation commands and server edits
remain rejected.

`format_buffer(&snapshot, &FormattingOptions { tab_size, insert_spaces })` returns
validated, nonoverlapping `TextEdit` ranges. `TextEdit::apply_all(original, &edits)`
converts zero-based UTF-16 positions to byte offsets without splitting surrogate
pairs, and returns a new string. It never saves. Check the result's snapshot
against the current draft before applying, and keep disk save explicit.

`set_formatter_configs(Vec<FormatterConfig>)` configures per-language stdin/stdout
formatters using `language`, `program`, `args`, without starting one. The supported
language IDs/aliases match existing server configuration. `{path}` and `{root}`
expand as single argv values; there is no shell expansion. Configure stdin-mode
arguments only, such as `--stdin-filepath {path}`; do not supply in-place/write
flags. An explicit configured formatter takes precedence over LSP formatting,
shares cancellation/deadlines, bounds output and stderr, and returns a full-source
replacement edit. Without one, the current server must advertise document
formatting. Automatic formatting/saving is never enabled here; S14/S15/S17 toggles
and save behavior belong to the UI. S12 server paths/argv keep the existing
Navigator configuration mechanism; S16 consumes the explicit formatter setter.

The mock peer tests cover dirty/nonexistent drafts, UTF-16/CRLF ranges, full and
incremental synchronization, roots, cancellation/invalidation, cross-file drafts,
capabilities, malformed edits, diagnostics and the TS command whitelist, signature
parameter offsets, hover, candidate resolution and configured formatter failures.
The opt-in real TypeScript test also checks these editor features, changed-draft
diagnostics and unchanged disk. For an SDK supplied separately from the adapter,
set `POKARAK_TS_SDK` to a compatible TypeScript package directory; authored fixtures
link only this dependency and verify the server creates no additional dependencies,
logs or compiler output. TypeScript 7 no longer supplies the legacy `tsserver.js`
required by this adapter; the verified fixture used TypeScript 6.0.3. Existing real
JDT navigation was also revalidated; Kotlin imports were not run for this change.
