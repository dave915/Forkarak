use super::*;

fn normalized(language: &str) -> &str {
    match language {
        "ts" | "js/ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "kt" | "kts" => "kotlin",
        other => other,
    }
}
fn same_language(configured: &str, language: &str) -> bool {
    let configured = normalized(configured);
    let language = normalized(language);
    configured == language || configured == language_family(language)
}
pub(super) fn enabled(value: Option<&serde_json::Value>, language: &str) -> bool {
    let Some(value) = value.and_then(serde_json::Value::as_object) else {
        return true;
    };
    value
        .get(language)
        .or_else(|| value.get(language_family(language)))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}
pub(super) fn custom_override(value: Option<&serde_json::Value>, language: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    let owned;
    let value = if let Some(text) = value.as_str() {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
            return true;
        };
        owned = parsed;
        &owned
    } else {
        value
    };
    // Invalid config must surface its real validation error at the frontend;
    // never hide it behind a managed installation.
    value
        .as_object()
        .is_none_or(|map| map.keys().any(|key| same_language(key, language)))
}
pub(super) fn legacy_override(language: &str) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    legacy_override_at(
        &PathBuf::from(home).join(".config/pokarak/language-servers.json"),
        language,
    )
}
pub(super) fn legacy_override_at(path: &Path, language: &str) -> bool {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(e) => return e.kind() != std::io::ErrorKind::NotFound,
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return true;
    };
    value
        .get("servers")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|servers| {
            servers
                .iter()
                .filter_map(|s| s.get("language").and_then(serde_json::Value::as_str))
                .any(|l| same_language(l, language))
        })
}
pub(super) fn phase_label(phase: &InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Queued => "대기 중",
        InstallPhase::Resolving => "버전 확인 중",
        InstallPhase::Downloading => "다운로드 중",
        InstallPhase::Verifying => "체크섬 검증 중",
        InstallPhase::Extracting => "압축 해제 중",
        InstallPhase::Installing => "설치 중",
        InstallPhase::Activating => "새 버전 적용 중",
        InstallPhase::Complete => "설치 완료",
        InstallPhase::Cancelled => "취소됨",
        InstallPhase::Failed => "실패 · 재시도 가능",
    }
}
pub(super) fn progress_line(progress: &InstallProgress) -> String {
    let phase = phase_label(&progress.phase);
    if progress.phase == InstallPhase::Failed {
        // Earlier receipts stored the subprocess diagnostic in both fields.
        if progress.error.as_deref() == Some(progress.message.as_str()) {
            let status = progress
                .message
                .split_once("The requested URL returned error:")
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .and_then(|code| code.parse::<u16>().ok())
                .filter(|code| (400..600).contains(code));
            return match status {
                Some(code) => format!("{phase} · 다운로드 요청 실패 (HTTP {code})"),
                None => format!("{phase} · 세부 정보에서 이전 설치 오류를 확인하세요."),
            };
        }
        return format!("{phase} · {}", progress.message);
    }
    let bytes = bytes_label(progress);
    let mut label = format!("{phase} · {}", progress.message);
    if !bytes.is_empty() {
        label.push(' ');
        label.push_str(&bytes);
    }
    label
}

pub(super) fn bytes_label(progress: &InstallProgress) -> String {
    let downloaded = progress.downloaded_bytes;
    match progress.total_bytes.filter(|total| *total > 0) {
        Some(total) => format!(
            "{} / {} bytes · {}%",
            downloaded,
            total,
            downloaded
                .saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0)
                .min(100)
        ),
        None if downloaded > 0 => format!("{downloaded} bytes · 전체 크기 미제공"),
        _ => String::new(),
    }
}
