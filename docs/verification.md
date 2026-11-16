# Native macOS verification record

Recorded 2026-11-17 (Asia/Seoul). Pokarak is a Rust, GPUI + GPUI Kit macOS desktop application. This record separates test logs, root-reported interaction checks, source inspection and pending work. It is not a web or iOS verification report.

The documentation pass inspected `PRODUCT.md`, `DESIGN.md`, `README.md` and the app source. The documentation worktree starts at `d46873b8b96a62c39de81b41abf6d3a9f3cb953f`; its app source is older than the implementation documented here. The updated `main.rs`, `prefs.rs`, `theme.rs`, settings/sidebar methods and reader/comparison rendering were read from `/Users/han/dev/pokarak`. They include applied comparison state, full diff lines, light/dark palettes, System appearance, persisted settings and the Worktrees/Branches selector. At inspection these root changes were uncommitted. The sidecar's `provenance.sourceSha256` records all five app source files; `ui.rs` was `bd1eba55a47b35713c1a93c4a5db1ab2d10dd7067257e6b6c092c832b13dd997` and `theme.rs` was `3f4261c2f1fc9b9f2c438258b361a2eec801827f9af62dd0590726bdbe1d0bc8`. These identify inspected source, not a release binary.

Only `.impeccable/design.json` and this record are changed by this feature. No application build, test suite or native UI session was run by this documentation pass.

The sidecar is refreshed from the implemented theme tokens and settings/sidebar source. Final visual receipts remain pending from root. Source inspection confirms implementation details; it does not establish that the new controls, persistence or appearance transitions passed a native interaction check.

## Automated evidence

The existing logs were read directly. The ordinary suite uses the `pokarak-navigation/test-server` feature; “default” here means non-ignored tests, not Cargo's default workspace-member selection.

| Suite | Passed | Evidence and scope |
| --- | ---: | --- |
| Core | 22 | `/tmp/pokarak-test.log`: real temporary Git fixture repositories; comparison, paths, worktrees, limits, search and read-only behavior. |
| Navigation | 23 | Same log: 3 unit tests and 20 stdio integration tests, including mock-server lifecycle, cancellation, import errors and isolation. |
| App | 2 | Same log and `/tmp/pokarak-final-app-tests.log`: CLI project/window-size parsing and invalid/literal arguments. These are not rendering or interaction tests; the second log is a repeat, not two additional tests. |
| **Default total** | **47** | **22 + 23 + 2, with zero failures.** The 3 real JVM tests and 1 real TS test are ignored in this run. |
| Real JVM, opt-in | 3 | `/tmp/pokarak-real-jvm.log`: JDT Java 17, official Kotlin LSP mixed Maven, and official Kotlin LSP Kotlin-only Maven fixtures. Zero failures. |
| Real TypeScript, opt-in | 1 | `/tmp/pokarak-real-ts.log`: `real_typescript_navigation_and_disk_updates`. Zero failures. |

The real-server results are separate from the 47-test default total. The logs originate from `/Users/han/dev/pokarak` and do not embed an exact source commit or release checksum. They are existing evidence, not a fresh test pass over the later UI edits. `/tmp` logs are local, ephemeral evidence; the counts and scope above remain the durable record.

### Real language-server scope

| Operation | Recorded result |
| --- | --- |
| Java call → Java definition; Java symbol → Java callers | Passed with real JDT LS in the Java 17 fixture. |
| Kotlin call → Kotlin definition and Kotlin callers | Passed in Kotlin-only and mixed Maven fixtures. |
| Kotlin call → Java definition | Passed in the mixed Maven fixture. |
| References requested at a Kotlin symbol include a Java caller | Passed in the mixed Maven fixture. |
| Java source → Kotlin definition | **Not established.** The optional Kotlin-server Java-source probe returned no locations; passing the enclosing test does not assert this capability. |
| Java symbol → Kotlin callers | **Not established.** JDT reference results explicitly warn `JDT LS Java scope: Kotlin callers may be missing`. Kotlin-symbol results do not prove Java-source coverage. |
| TS/TSX definition; JSX → JS definition; TS references and implementation | Passed in the real TS test. |
| TS disk update and independent fixture-root navigation | Passed in the real TS test; this does not verify the app's filesystem notification UI. |
| Gradle import, Spring-specific navigation, large enterprise multimodule projects, generated/custom source layouts | **Not verified.** Maven fixture success does not establish these behaviors. |

The tests use authored temporary repositories. Real Kotlin tests explicitly allow Maven project import, which can run Maven/plugins and resolve dependencies. They install no language servers; setup is a separate explicit operation. See [language analysis](language-analysis.md) and [navigation documentation](../crates/navigation/README.md) for tools, configuration and limits.

## Native UI matrix

“Root-reported checked” below records the supplied report from operating the actual macOS app. It does not imply a screenshot review by this documentation pass, exhaustive language coverage, exact pixel/scroll restoration, or a new check of later source changes. Final screenshots and their window sizes remain pending.

| Surface / flow | Reproduction and observation to record | Current evidence |
| --- | --- | --- |
| Source → definition | In current code, with explicit analysis trust, Cmd-click a call/reference and inspect the target. | Root-reported checked in the native app. |
| Declaration → usages | Cmd-click the declaration; inspect the usages panel and its semantic label. | Root-reported checked. Backend language limits above still apply. |
| Back | After navigating, use Back / Cmd-[ and inspect the previous reading location. | Root-reported checked. Forward and exact scroll/cursor fidelity need a separate recorded check. |
| Worktree switch | Select another worktree and return; inspect worktree identity and reading context. | Root-reported checked; race/stale-response stress is not claimed. |
| Worktrees / Branches sidebar selector | Switch sidebar views and restart to inspect the saved mode. Select a branch as the comparison base; confirm worktree identity and checked-out branch do not change, and distinguish selected base from the `현재` marker. | **Source confirmed; native interaction/persistence receipts pending.** `show_branches` is global. The branch callback sets the base and calls comparison, with a worktree-scope guard and no checkout. |
| Branch compare | Choose a valid comparison base and inspect branch changes. | Root-reported checked before the later applied-base controls update. |
| Applied base and pending draft | In branch mode edit the base; confirm `적용 *`, while the applied label still reflects the installed snapshot. Apply or press Enter; check the label after success and retention after an invalid-base failure. | **Source inspected; updated interaction check pending.** Pending marker is set only when an applied branch base differs from the trimmed draft. |
| Keyboard filename filter | Cmd-P, type part of a filename/path, use arrows and Enter. | Root-reported checked. |
| Multiple projects | Switch projects and inspect their separate worktree/file/search state. | Root-reported checked. Restart persistence and exhaustive combinations are not claimed. |
| Close all → empty | File menu → 모든 프로젝트 닫기; inspect the welcome/open-project surface. | Root-reported checked. |
| Native titlebar and fonts | Inspect traffic lights, continuous titlebar/project-tab background, Korean text, bundled UI/code fonts, path clipping and focus. | Implementation inspected; final visual evidence pending. |
| Settings: System / Light / Dark | Open Settings with Cmd-, / titlebar / app menu. Select each appearance, inspect native surfaces, change OS appearance in System mode, then restart to inspect persistence. | **Source confirmed; final visual/interaction receipts pending.** Explicit Palette mappings, `Preferences.theme`, immediate saves and the System-only window-appearance observer are implemented. Default is Dark. |
| Settings: code font size | Use `코드 보기 글자 크기` at 10, 13 and 24; check boundary buttons, code text and restart persistence. Confirm diff stays 12px with 22px rows and UI remains 13px. | **Source confirmed; native interaction/persistence receipts pending.** Global `code_font_size` applies to whole-file code only; range 10–24, step 1, effective default 13. |
| Default and minimum windows; resize handles | Inspect 1440×900 and 940×580; drag pane dividers and check controls, results and long paths. | Sizes/ranges confirmed in source; a complete size-specific visual pass is pending. |
| Diff / code and long lines | Toggle modes and horizontally scroll a diff line longer than 2,000 characters, including removed content. | Updated full-line rendering confirmed in source; native long-line interaction check pending. |
| Results keyboard navigation / text fallback | Content search or usages: verify label, arrows, Enter, Escape and empty results. | Source inspected; dedicated native UI check pending beyond reported declaration-to-usages. |
| Loading, error, missing worktree, deleted/binary/limited file | Trigger each state with disposable fixtures; verify honest messages and disabled navigation where applicable. | Source and backend evidence only; complete native state matrix pending. |
| External change while reading | Modify a disposable fixture externally, inspect notification, refresh and location retention; reject stale navigation. | Source and backend evidence only; native notification/retention check pending. |
| Accessibility | Check keyboard focus, VoiceOver labels/actions, contrast and pane navigation in macOS. | AccessKit roles/labels and key bindings exist in source; no complete accessibility pass recorded. |

## Source-derived UI contract

The [design sidecar](../.impeccable/design.json) contains the actual app palette, font roles, spacing helpers, explicit dimensions, titlebar, panes, component states and modes. Its `extensions.native` block is a deliberate native extension of the Impeccable version-2 envelope: `DESIGN.md` has no token frontmatter and remains unchanged. Native component descriptions replace web preview snippets. No synthetic color ramps or unimplemented breakpoints/motion tokens are presented as runtime design facts.

Implemented source details, including differences from the compact prose in `DESIGN.md`:

- The main panes initially allocate 210px to the Worktrees/Branches sidebar and **285px** to files; their declared resize ranges are 160–420px and 200–600px. The reader takes remaining width. The sidebar selector uses 12px labels in a 44px header; branch rows are 32px high.
- The native titlebar is 52px, project tabs 38px, comparison bar 42px, panel headers 44px and footer 27px. Native traffic lights sit at `(15, 18)`; titlebar content reserves 90px on the left.
- File rows are 29px, diff rows 22px, and result/recent rows 40px. Worktree rows follow content and padding. Whole-file code uses JetBrains Mono with the persisted `code_font_size`; diff stays at 12px. The previous worktree title/count header has been replaced by the Worktrees/Branches selector.
- GPUI Kit 0.10.0 sets native rem size from the 13px theme font. Helpers `_1` through `_6` represent 0.25/0.5/0.75/1/1.25/1.5 rem, resolving from source to 3.25/6.5/9.75/13/16.25/19.5 logical pixels. Small buttons/inputs use `h_6`, not a hardcoded 28px. These are source-derived values, not screenshot measurements. Explicit `px(...)` sizes are independent of this scale.
- The explicit 5px corner radius applies to worktree rows and project-tab wrappers. GPUI Kit owns button/input radii, focus/disabled styling, syntax colors and library effects; the app has no separate shadow or motion scale.
- `theme.rs::Palette` preserves dark values and explicitly maps all 19 app color roles to light values. `theme::apply` maps the app theme overrides after selecting the GPUI Kit theme. Default appearance is Dark; System resolves `window.appearance()` and subscribes to appearance changes only while System is selected. The sidecar records both palettes without inventing a separate System palette.
- Settings is a 430px GPUI Kit dialog, reached through the titlebar, app menu or Cmd-,. Theme selection and font-size changes save immediately. `코드 보기 글자 크기` changes whole-file code only, with default 13, range 10–24 and step 1; missing/invalid/out-of-range saved sizes normalize to 13 at startup. The displayed suffix is `pt`, while rendering uses GPUI logical `px(...)`. UI size remains 13px; diff and metadata sizes remain explicit.
- `theme`, `code_font_size` and `show_branches` are global preferences in the same external `session.json` used for saved views. `show_branches` defaults to false (Worktrees). Branch rows select an applied comparison base via `base_input` and `compare(true)` without checkout or changing worktree; `현재` marks the checked-out branch independently of the selected base. Comparison state itself remains per worktree.
- Applied comparison labels derive from `snapshot.compare`; edited draft text does not silently relabel existing changes. Apply/Enter starts the comparison request, and refresh failure keeps the installed snapshot.
- Diff rows now render the full nonempty `line.text` with horizontal scrolling. There is **no app-level 2,000-character per-line truncation**. Core text-preview size, diff line-count and calculation limits remain, with partial-display notices. See [core limits](../crates/core/README.md).

## Reproduction commands

These are commands for a later rerun, not additional claimed results. Run from the intended source checkout. Keep the shared target directory. Long Cargo operations use Herdr's installed `hc run --bg` / `hc wait` workflow; record the exit result and then collect output. Wait again if `hc wait` reports a timeout/message rather than process completion.

```sh
hc run --bg --name pokarak-default-check --cwd "$PWD" -- /usr/bin/env \
  PATH="/Users/han/.cargo/bin:$PATH" \
  CARGO_TARGET_DIR=/Users/han/dev/pokarak/target \
  cargo test --workspace --features pokarak-navigation/test-server
hc wait pokarak-default-check
hc read pokarak-default-check --lines 200

# Separately opt in to the already-installed real servers.
hc run --bg --name pokarak-jvm-check --cwd "$PWD" -- /usr/bin/env \
  PATH="/Users/han/.cargo/bin:$PATH" \
  CARGO_TARGET_DIR=/Users/han/dev/pokarak/target \
  cargo test -p pokarak-navigation --test jvm -- --ignored --nocapture --test-threads=1
hc wait pokarak-jvm-check
hc read pokarak-jvm-check --lines 200

hc run --bg --name pokarak-ts-check --cwd "$PWD" -- /usr/bin/env \
  PATH="/Users/han/.cargo/bin:$PATH" \
  CARGO_TARGET_DIR=/Users/han/dev/pokarak/target \
  cargo test -p pokarak-navigation --test typescript -- --ignored --nocapture
hc wait pokarak-ts-check
hc read pokarak-ts-check --lines 200
```

Managed tools default to `~/.local/share/pokarak/tools`; the real tests also accept `POKARAK_TOOLS_DIR`. The TS test additionally accepts `POKARAK_TS_SERVER`. If tools are absent, follow the explicit setup instructions in [language analysis](language-analysis.md); ordinary file viewing does not require setup or trust.

Use an existing bundle for UI reproduction, so launching performs no Cargo build. Replace example project paths with disposable fixture repositories. A fresh `--session-dir` isolates UI state and trust from the normal session. These are separate launches; close the previous QA instance before changing cases.

```sh
POKARAK_APP='/Users/han/dev/pokarak/dist/포카락.app'

# Empty/welcome at the default size, using a fresh session directory.
open -n "$POKARAK_APP" --args \
  --window-size 1440x900 --session-dir /tmp/pokarak-qa-doc-empty

# Small-window, multiple-project case.
open -n "$POKARAK_APP" --args \
  --window-size 940x580 --session-dir /tmp/pokarak-qa-doc-small \
  -- /absolute/path/to/backend-fixture /absolute/path/to/frontend-fixture

# Existing-bundle inspection; does not rebuild or re-sign.
du -sh "$POKARAK_APP"
plutil -lint "$POKARAK_APP/Contents/Info.plist"
lipo -archs "$POKARAK_APP/Contents/MacOS/pokarak"
codesign --verify --deep --strict "$POKARAK_APP"
```

Bundle reproduction options, including packaging an existing binary without compiling it, are in [packaging](packaging.md). Do not infer build or signing results merely from the presence of a command above.

Documentation-only validation:

```sh
python3 -m json.tool .impeccable/design.json >/dev/null
git diff --check -- .impeccable/design.json docs/verification.md
```

This pass validated JSON syntax and duplicate-key absence, resolved token references, matched all 19 dark/light palette pairs against `Palette::rgb`, checked source hashes, spacing arithmetic and font/link paths, and reconciled the test totals against the existing logs. These are documentation consistency checks, not new application tests.

## Release evidence and remaining work

The root reported that the native source `.app` release built successfully, the bundle is **24MB**, and `codesign --verify --deep --strict` passed. `/tmp/pokarak-release.log` independently records completion of the optimized release build and packaging to `/Users/han/dev/pokarak/dist/포카락.app`. Bundle size and the separate deep/strict verification are root-reported; this documentation pass did not rerun them. Local signature verification does not establish Developer ID signing or notarization.

That release report preceded the settings/sidebar source update. A release-to-source association and visual receipts for the new implementation are still pending; the earlier build/signature result is not promoted to a fresh release verification of these changes.

| Remaining evidence | Status |
| --- | --- |
| Final screenshots with window sizes, scenario labels and artifact paths | Pending from root. |
| Worktrees/Branches selector, System/Light/Dark settings and persisted code font size | Implementation and palette values inspected; final native visual/interaction/persistence receipts pending from root. |
| Exact release source revision, binary/bundle checksum and association with final screenshots | Pending from root. |
| Recheck applied-base/pending behavior and full long-line diff scrolling after the source update | Pending native UI evidence. |
| Measured startup, navigation, scrolling, indexing, memory or large-project performance | **No measured benchmark.** Cargo/test log durations are not application performance measurements. |
| Java-source → Kotlin and complete JDT Kotlin-caller coverage | Not established; retain the limits above. |
| Gradle/Spring-specific behavior and production-scale projects | Not verified. |
| Minimum macOS 13 runtime, Intel Mac, other OS packaging, Developer ID/notarization/update delivery | Not verified by this record. |

Add screenshots and release identifiers only when supplied or observed; keep untested matrix rows pending rather than turning source implementation into a UI pass.
