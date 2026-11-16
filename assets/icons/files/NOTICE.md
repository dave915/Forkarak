# File identity icons

All 20 SVGs in this directory are original geometric drawings authored for
Pokarak. Copyright (c) 2026 Pokarak contributors. They are distributed under the
repository's [MIT license](../../../LICENSE). No external SVG, font, raster,
icon library, or network resource is incorporated. These are file identity
glyphs, not official brand artwork; the language and project names identify
their respective file formats.

Each drawing uses a 16 × 16 viewBox, explicit vector geometry, and currentColor.
The app embeds the SVG bytes at compile time and displays them at 14 px in both
file rows and small tab buttons. GPUI tints the SVG mask using the existing
palette's contrasting light/dark colors. Shape identifies the file type even
without color. Git status letters retain their own colors independently.

| Asset | File mapping / visual cue |
| --- | --- |
| java.svg | `.java`; steaming cup |
| kotlin.svg | `.kt`, `.kts`; angular K |
| javascript.svg | `.js`, `.mjs`, `.cjs`; boxed JS |
| typescript.svg | `.ts`, `.mts`, `.cts` (including `.d.ts`); boxed TS |
| react.svg | `.jsx`, `.tsx`; atom |
| vue.svg | `.vue`; nested V |
| svelte.svg | `.svelte`; linked S |
| html.svg | `.html`, `.htm`; shield with angle brackets |
| css.svg | `.css`; shield with 3 |
| scss.svg | `.scss`, `.sass`; looping S |
| json.svg | `.json`, `.jsonc`, `.json5`; braces and colon |
| yaml.svg | `.yaml`, `.yml`; Y with list lines |
| xml.svg | `.xml`; angle brackets and slash |
| maven.svg | `pom.xml`; M with feather |
| gradle.svg | `.gradle`, `*.gradle.kts`, `gradle.properties`, `gradle-wrapper.properties`, `gradlew`, `gradlew.bat`; elephant |
| rust.svg | `.rs`; toothed ring with R |
| cargo.svg | `Cargo.toml`, `Cargo.lock`; package |
| markdown.svg | `.md`, `.markdown`, `.mdx`; M with down arrow |
| gitignore.svg | `.gitignore`; branch with exclusion cross |
| file.svg | Unknown/extensionless files; folded document |

Matching is ASCII case-insensitive, based only on the filename, with special
filenames and compound Gradle suffixes taking precedence over extensions.
Directories retain the existing folder icon. Files with non-UTF-8 names use
the generic document icon. No file contents or filesystem metadata are read.
