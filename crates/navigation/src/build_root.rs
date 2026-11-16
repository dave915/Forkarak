//! Filesystem-only discovery. Never evaluate build scripts or walk outside the worktree.
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub(crate) fn discover(root: &Path, source: &Path, jdt: bool) -> Result<PathBuf> {
    let mut nearest = None;
    let mut enclosing = None;
    for dir in source.parent().into_iter().flat_map(Path::ancestors) {
        if !dir.starts_with(root) {
            break;
        }
        let has = |name: &str| {
            let path = dir.join(name);
            path.is_file() && path.canonicalize().is_ok_and(|p| p.starts_with(root))
        };
        let settings = has("settings.gradle") || has("settings.gradle.kts");
        let maven = has("pom.xml");
        if settings
            || maven
            || has("build.gradle")
            || has("build.gradle.kts")
            || (jdt && has(".project") && has(".classpath"))
        {
            nearest.get_or_insert_with(|| dir.to_path_buf());
        }
        // A settings project or a Maven reactor enclosing the selected module
        // wins over a leaf. Outermost enclosing build wins over nested reactors.
        if settings || (maven && contains_module(dir, source)) {
            enclosing = Some(dir.to_path_buf());
        }
        // Do not import through a nested Git repository boundary either.
        if dir != root && dir.join(".git").exists() {
            break;
        }
    }
    if let Some(path) = enclosing.or(nearest) {
        return Ok(path);
    }
    bail!(
        "noBuildSystemFound: Maven/Gradle 프로젝트 설정이 없습니다. 선택한 소스의 상위 폴더(현재 워크트리 안)에 pom.xml 또는 Gradle 빌드 설정이 있는지 확인하세요.{}\nWorktree: {}\nSource: {}",
        if jdt {
            " JDT는 .project와 .classpath가 있는 Eclipse 프로젝트도 지원합니다."
        } else {
            ""
        },
        root.display(),
        source.display()
    )
}

fn contains_module(dir: &Path, source: &Path) -> bool {
    let Ok(xml) = std::fs::read_to_string(dir.join("pom.xml")) else {
        return false;
    };
    let Ok(doc) = roxmltree::Document::parse(&xml) else {
        return false;
    };
    doc.descendants()
        .filter(|n| {
            n.has_tag_name("module") && n.parent().is_some_and(|p| p.has_tag_name("modules"))
        })
        .filter_map(|n| n.text())
        .any(|module| {
            dir.join(module.trim())
                .canonicalize()
                .is_ok_and(|path| path != dir && path.starts_with(dir) && source.starts_with(path))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    #[test]
    fn nested_builds_prefer_enclosing_reactors_and_settings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("backend/api/src/Main.kt");
        write(&root, "backend/api/pom.xml", "<project/>");
        assert_eq!(
            discover(&root, &source, false).unwrap(),
            root.join("backend/api")
        );
        write(
            &root,
            "backend/pom.xml",
            "<project><modules><module>api</module></modules></project>",
        );
        assert_eq!(
            discover(&root, &source, false).unwrap(),
            root.join("backend")
        );
        // An unrelated root POM is not an enclosing aggregator.
        write(
            &root,
            "pom.xml",
            "<project><!-- <modules><module>backend</module></modules> --></project>",
        );
        assert_eq!(
            discover(&root, &source, false).unwrap(),
            root.join("backend")
        );
        write(&root, "settings.gradle.kts", "include(\":backend:api\")");
        assert_eq!(discover(&root, &source, false).unwrap(), root);
    }
    #[test]
    fn bounds_missing_metadata_eclipse_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().canonicalize().unwrap();
        let root = parent.join("worktree");
        write(&parent, "pom.xml", "<project/>");
        write(&root, "backend/src/Main.java", "class Main {}");
        let source = root.join("backend/src/Main.java");
        assert!(
            discover(&root, &source, false)
                .unwrap_err()
                .to_string()
                .contains("noBuildSystemFound")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(parent.join("pom.xml"), root.join("pom.xml")).unwrap();
            assert!(discover(&root, &source, false).is_err());
        }
        write(&root, "backend/.project", "");
        write(&root, "backend/.classpath", "");
        assert_eq!(
            discover(&root, &source, true).unwrap(),
            root.join("backend")
        );
        assert!(discover(&root, &source, false).is_err());
    }
}
