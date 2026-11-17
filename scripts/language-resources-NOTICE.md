# Forkarak bundled language support

Java is included in this application: Eclipse JDT Language Server 1.63.0 and
Eclipse Temurin OpenJDK 25.0.5+8 (Apple Silicon macOS). No Homebrew, Python,
separately installed JDK or developer tools are required to run this support.
Language analysis and project import remain explicit application actions.
Project builds may need their own configured target JDK, SDK or build artifacts;
the included JDK is the server runtime, not a promise to build every project.

The upstream distributions are retained, including all included legal files,
licenses, copyrights and notices. JAR contents and their signatures are unchanged.
Filesystem Mach-O code is signed for this application; no Java source is modified.
Original binary archive hashes are recorded in provenance.json. Shipping file
hashes (after signing) are in ../SHA256SUMS. This app does not claim upstream
endorsement, Apple notarization or a warranty for third-party software.

## Java runtime and corresponding source

Eclipse Temurin is distributed under GPL version 2 with the Classpath Exception,
with additional third-party notices in java/runtime/Contents/Home/legal and the
runtime NOTICE. Those terms remain applicable. The complete matching upstream
OpenJDK source archive is included in language-tools/sources, along with the
exact Adoptium build-script source and official build metadata/SBOM. This includes
native sources; lib/src.zip is not used as a substitute for corresponding source.
SOURCE-ACCESS.md identifies the official exact versions, URLs and SHA-256 hashes.
See https://adoptium.net/docs/faq and the included runtime license files.

## JDT LS and transitive dependencies

JDT LS and most Eclipse components use EPL-2.0. JUnit includes EPL-1.0 material.
The original about.html/about_files and all detected embedded LICENSE/NOTICE
material are retained in each JAR and copied under legal/jdtls-jars for inspection.
jdtls-components.json inventories every top-level AND nested JAR with its exact
hash, version, source reference, Maven coordinates and included notices. Exact
Eclipse source commits and Maven source access are listed in SOURCE-ACCESS.md.
The JDT LS root source archive is also included.

Dependency terms include Apache-2.0, EPL-1.0/2.0, BSD-2/3-Clause, MIT and public
domain notices; they are not all covered by the JDT LS root EPL license. Logback
is used under its EPL option; JNA under its Apache-2.0 option. Original alternative
license notices are preserved. Jakarta components retain their upstream dual
license statements. The Eclipse m2e Maven runtime contains additional nested
components; its historical about.html lists older versions, so the inventory and
pinned supplemental sources identify the actual shipped versions separately.

Supplemental official sources/licenses are included for components whose binary
JAR lacks legal text, including Flexmark (BSD-2-Clause, Atlassian/Vladimir
Schneider), ASM (BSD-3-Clause, INRIA/France Telecom), JetBrains annotations and
Fernflower Java decompiler (Apache-2.0, JetBrains and contributors), Gson,
Logback, AOP Alliance, Sisu and Plexus components. Fernflower's exact official
source JAR contains Apache-2.0 copyright headers; this grant concerns that
component only and does not authorize redistributing Kotlin's proprietary server.
Supplemental source copyright and redistribution headers are reproduced under
legal/source-copyrights. Full source JARs remain in sources. Full Apache and
Eclipse license texts plus the Flexmark copyright/license are included here.
The pinned lock records the complete reviewed JAR set; dependency changes fail
preparation until this distribution review and corresponding material are updated.

## Kotlin: official first-use download, not bundled

This application includes only Kotlin installation metadata/recipe. It does not
redistribute the Kotlin server, JetBrains Runtime or other Kotlin binary content.
The official Kotlin README states that the server contains proprietary parts of
JetBrains Air/Fleet and is partially closed-source. Its repository Apache license
does not establish a complete-binary redistribution grant.

The native app's explicit Install action downloads the unmodified official Kotlin
standalone distribution and verifies the vendor checksum. The distribution's own
terms and included notices apply and are exposed by the manager. There is no
additional Forkarak acceptance gate. No paid unified Java/Kotlin server is used.
See https://github.com/Kotlin/kotlin-lsp#project-status and
https://github.com/Kotlin/kotlin-lsp/releases/tag/kotlin-lsp/v271.2085.0 .
