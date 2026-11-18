//! Exercise the normal first-use recipe, verification, activation and attempt record
//! with small authored upstream responses and no external servers or installed JVM.
use super::*;
use sha2::{Digest, Sha256, Sha512};
use std::os::unix::fs::PermissionsExt;

fn archive(executable: &str) -> Vec<u8> {
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    let bytes = b"fixture executable";
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, executable, &bytes[..])
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap()
}

fn responses() -> Vec<(String, Vec<u8>)> {
    let arm = cfg!(target_arch = "aarch64");
    let kotlin_file = if arm {
        "kotlin-aarch64.sit"
    } else {
        "kotlin.sit"
    };
    let jdk_arch = if arm { "aarch64" } else { "x64" };
    let jdk_file = format!("OpenJDK25U-jdk_{jdk_arch}_mac_hotspot_fixture.tar.gz");
    let kotlin = format!("https://download.jetbrains.com/kotlin/{kotlin_file}");
    let jdk = format!(
        "https://github.com/adoptium/temurin25-binaries/releases/download/fixture/{jdk_file}"
    );
    let base = "https://repo.maven.apache.org/maven2/org/apache/maven/apache-maven";
    let maven = format!("{base}/3.9.9/apache-maven-3.9.9-bin.tar.gz");
    let kotlin_archive = archive("kotlin/bin/intellij-server");
    let jdk_archive = archive("jdk/Contents/Home/bin/java");
    let maven_archive = archive("maven/bin/mvn");
    vec![
        ("https://api.github.com/repos/Kotlin/kotlin-lsp/releases/latest".into(),
         serde_json::to_vec(&serde_json::json!({"tag_name":"v1.2.3","body":kotlin})).unwrap()),
        (format!("{kotlin}.sha256"), format!("{:x}", Sha256::digest(&kotlin_archive)).into_bytes()),
        (kotlin, kotlin_archive),
        ("https://api.github.com/repos/adoptium/temurin25-binaries/releases/latest".into(),
         serde_json::to_vec(&serde_json::json!({"tag_name":"fixture","assets":[{"name":jdk_file,"browser_download_url":jdk}]})).unwrap()),
        (format!("{jdk}.sha256.txt"), format!("{:x}", Sha256::digest(&jdk_archive)).into_bytes()),
        (jdk, jdk_archive),
        (format!("{base}/maven-metadata.xml"), b"<versions><version>3.9.9</version></versions>".to_vec()),
        (format!("{maven}.sha512"), format!("{:x}", Sha512::digest(&maven_archive)).into_bytes()),
        (maven, maven_archive),
    ]
}

#[test]
fn kotlin_first_use_installs_all_three_tools_or_records_the_exact_refused_request() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().canonicalize().unwrap();
    let curl = fixture.join("curl");
    fs::write(&curl, r#"#!/bin/sh
set -eu
root=$(dirname "$0")
while [ "$#" -gt 0 ]; do
    case "$1" in
        --url) url=$2; shift ;;
        --output) output=$2; shift ;;
        --dump-header) headers=$2; shift ;;
    esac
    shift
done
index=$(wc -l < "$root/requests")
printf '%s\n' "$url" >> "$root/requests"
test "$url" = "$(sed -n "$((index+1))p" "$root/urls")" || exit 91
if [ "$index" -eq "$(cat "$root/refused")" ]; then
    printf 'HTTP/2 302\r\nLocation: https://cdn.invalid/private?signature=SECRET\r\n\r\nHTTP/2 403\r\n\r\n' > "$headers"
    echo 'fixture transfer refused' >&2
    echo 'https://user:password@cdn.invalid/private?signature=SECRET' >&2
    echo 'X-Amz-Credential: SECRET' >&2
    # A received HTTP refusal still wins if the subprocess exits successfully.
    if [ "$index" -eq 6 ]; then exit 0; fi
    exit 22
fi
printf 'HTTP/2 200\r\n\r\n' > "$headers"
cat "$root/body-$((index+0))" > "$output"
"#).unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    let responses = responses();
    let urls: Vec<_> = responses.iter().map(|(url, _)| url.as_str()).collect();
    fs::write(fixture.join("urls"), urls.join("\n")).unwrap();
    for (index, (_, body)) in responses.iter().enumerate() {
        fs::write(fixture.join(format!("body-{index}")), body).unwrap();
    }

    // Each run starts with an empty cache and manifest, even when a preceding
    // component succeeded in another case. The final case accepts every request.
    for refused in 0..=urls.len() {
        let tools = ManagedTools::new(fixture.join(format!("install-{refused}")), None).unwrap();
        let mut control = InstallControl::new("kotlin");
        control.curl = Some(curl.clone());
        fs::write(fixture.join("requests"), "").unwrap();
        fs::write(fixture.join("refused"), refused.to_string()).unwrap();
        let outcome = tools.install("kotlin", &control);
        let attempt = tools.root().join("attempts/kotlin.json");
        control.finish(outcome, &attempt);
        let durable = tools.last_attempt("kotlin").unwrap().unwrap();
        let requests = fs::read_to_string(fixture.join("requests")).unwrap();
        assert_eq!(
            requests.lines().collect::<Vec<_>>(),
            urls[..(refused + 1).min(urls.len())]
        );
        if refused == urls.len() {
            let receipt = control.wait().unwrap();
            assert_eq!(receipt.distribution, ToolDistribution::FirstUseDownload);
            assert_eq!(receipt.artifacts.len(), 3);
            assert_eq!(receipt.servers[0].language, "kotlin");
            assert_eq!(durable.phase, InstallPhase::Complete);
            let status = tools.status("kotlin").unwrap();
            assert!(status.problem.is_none());
            assert_eq!(status.installed.unwrap().version, receipt.version);
        } else {
            let component = ["Kotlin LSP", "Temurin JDK", "Maven"][refused / 3];
            let stage = ["메타데이터", "체크섬", "압축 파일"][refused % 3];
            let url = url::Url::parse(urls[refused]).unwrap();
            let summary = &durable.message;
            for expected in [
                component,
                stage,
                url.host_str().unwrap(),
                "HTTP 403",
                "거부",
            ] {
                assert!(summary.contains(expected), "{summary}");
            }
            let diagnostic = durable.error.unwrap();
            assert_eq!(durable.phase, InstallPhase::Failed);
            assert_eq!(control.wait().unwrap_err().to_string(), diagnostic);
            assert_eq!(control.snapshot().message, *summary);
            assert_ne!(&diagnostic, summary);
            if refused == 6 {
                assert!(diagnostic.contains("unsuccessful HTTP response"));
            } else {
                assert!(diagnostic.contains("curl failed (exit code 22)"));
                assert!(diagnostic.contains("fixture transfer refused"));
            }
            let raw = fs::read_to_string(attempt).unwrap();
            for private in [
                "Language server exited",
                "SECRET",
                "password",
                "cdn.invalid",
                "https://",
            ] {
                assert!(!raw.contains(private), "{raw}");
            }
            assert!(tools.status("kotlin").unwrap().installed.is_none());
        }
    }
}
