//! Settings metadata shared by the native dialog and runtime consumers.
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Category {
    Appearance,
    Editor,
    Language,
    Navigation,
    Review,
    Keyboard,
    Execution,
    Ai,
    Extensions,
    Updates,
}
impl Category {
    pub const ALL: [Self; 10] = [
        Self::Appearance,
        Self::Editor,
        Self::Language,
        Self::Navigation,
        Self::Review,
        Self::Keyboard,
        Self::Execution,
        Self::Ai,
        Self::Extensions,
        Self::Updates,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Editor => "editor",
            Self::Language => "language",
            Self::Navigation => "navigation",
            Self::Review => "review",
            Self::Keyboard => "keyboard",
            Self::Execution => "execution",
            Self::Ai => "ai",
            Self::Extensions => "extensions",
            Self::Updates => "updates",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "모양·시작 화면",
            Self::Editor => "에디터·들여쓰기",
            Self::Language => "언어 분석·포맷",
            Self::Navigation => "탐색·검색",
            Self::Review => "Git·검토",
            Self::Keyboard => "키보드",
            Self::Execution => "터미널·실행",
            Self::Ai => "AI 도구",
            Self::Extensions => "확장",
            Self::Updates => "업데이트",
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Bool,
    Choice(&'static [(&'static str, &'static str)]),
    Number(usize, usize),
    Text,
    Json,
}
#[derive(Clone, Copy)]
pub(super) struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub category: Category,
    pub kind: Kind,
    pub app_only: bool,
}
macro_rules! field {
    ($cat:ident,$key:literal,$label:literal,$hint:literal,$kind:expr) => {
        Field {
            key: $key,
            label: $label,
            hint: $hint,
            category: Category::$cat,
            kind: $kind,
            app_only: false,
        }
    };
}
pub(super) const FIELDS: &[Field] = &[
    field!(
        Appearance,
        "workspace.default_view",
        "기본 작업 화면",
        "저장된 작업이 없을 때 여는 화면입니다.",
        Kind::Choice(&[("editor", "에디터"), ("git", "Git")])
    ),
    field!(
        Appearance,
        "workspace.restore_last",
        "마지막 작업 복원",
        "워크트리별 화면·파일·커서·읽던 위치를 복원합니다.",
        Kind::Bool
    ),
    field!(
        Appearance,
        "layout.auto_collapse",
        "작은 창에서 패널 자동 접기",
        "코드와 diff 공간을 확보합니다.",
        Kind::Bool
    ),
    field!(
        Editor,
        "editor.editorconfig",
        ".editorconfig 적용",
        "프로젝트 규칙을 기본 들여쓰기보다 우선합니다.",
        Kind::Bool
    ),
    field!(
        Editor,
        "editor.detect_indent",
        "파일 들여쓰기 감지",
        "기존 파일에서 탭과 공백 규칙을 감지합니다.",
        Kind::Bool
    ),
    field!(
        Editor,
        "editor.tab_size",
        "기본 탭 크기",
        "프로젝트·파일 규칙이 없을 때 적용합니다.",
        Kind::Number(1, 16)
    ),
    field!(
        Editor,
        "editor.use_spaces",
        "공백으로 들여쓰기",
        "끄면 탭 문자를 사용합니다.",
        Kind::Bool
    ),
    field!(
        Editor,
        "editor.word_wrap",
        "긴 줄 화면 줄바꿈",
        "파일 내용에 줄바꿈 문자를 넣지 않습니다.",
        Kind::Bool
    ),
    field!(
        Editor,
        "editor.wrap_patterns",
        "파일 종류별 줄바꿈",
        "JSON: {\"rs\":false,\"md\":true} · 확장자별 재정의",
        Kind::Json
    ),
    field!(
        Editor,
        "editor.preview_tabs",
        "미리보기 탭 사용",
        "편집하거나 고정하면 일반 탭이 됩니다.",
        Kind::Bool
    ),
    field!(
        Editor,
        "editor.inline_changes",
        "코드 안 변경 줄 표시",
        "변경 표시에서 이전 코드를 펼칩니다.",
        Kind::Bool
    ),
    field!(
        Language,
        "language.servers",
        "언어 서버 경로·인수",
        "JSON: {\"rust\":{\"path\":\"rust-analyzer\",\"args\":[]}}",
        Kind::Json
    ),
    field!(
        Language,
        "language.completion",
        "자동완성",
        "분석을 명시적으로 허용하고 켠 경우에만 사용합니다.",
        Kind::Bool
    ),
    field!(
        Language,
        "language.completion_on_input",
        "입력 중 자동완성",
        "언어 분석을 켠 편집기에서 입력하면 후보를 표시합니다. 수동 호출은 ⌃Space입니다.",
        Kind::Bool
    ),
    field!(
        Language,
        "language.auto_signature_help",
        "입력 중 함수 도움말",
        "여는 괄호와 쉼표를 입력하면 함수 인자를 표시합니다.",
        Kind::Bool
    ),
    field!(
        Language,
        "language.diagnostics",
        "오류 표시",
        "분석을 켠 경우 진단을 표시합니다.",
        Kind::Bool
    ),
    field!(
        Language,
        "language.unsaved",
        "미저장 코드 탐색",
        "현재 표시 중인 초안으로 정의·사용처를 찾습니다.",
        Kind::Bool
    ),
    field!(
        Language,
        "formatters.config",
        "언어별 포맷터",
        "JSON: {\"rust\":{\"path\":\"rustfmt\",\"args\":[]}} · 표준입력/출력",
        Kind::Json
    ),
    field!(
        Language,
        "editor.format_on_save",
        "저장 시 포맷",
        "기본은 수동 실행입니다. 허용된 프로젝트에만 적용합니다.",
        Kind::Bool
    ),
    field!(
        Navigation,
        "navigation.results_placement",
        "탐색 결과 표시",
        "정의와 사용처를 표시할 위치입니다.",
        Kind::Choice(&[
            ("editor", "편집기 탭"),
            ("popover", "팝오버"),
            ("files", "파일 영역"),
            ("bottom", "하단")
        ])
    ),
    field!(
        Navigation,
        "navigation.blame_mode",
        "Blame 표시",
        "현재 줄부터 확인하거나 파일 전체에 표시합니다.",
        Kind::Choice(&[("line", "현재 줄"), ("file", "파일 전체")])
    ),
    field!(
        Navigation,
        "navigation.symbols_placement",
        "심볼 목록 위치",
        "파일 심볼의 기본 표시 위치입니다.",
        Kind::Choice(&[("sidebar", "사이드바"), ("popover", "팝오버")])
    ),
    field!(
        Navigation,
        "search.exclude",
        "프로젝트 검색 제외 패턴",
        "줄마다 glob 패턴 하나. 예: **/node_modules/**",
        Kind::Text
    ),
    field!(
        Review,
        "review.split_diff",
        "기본 diff 방식",
        "각 워크트리에서 마지막 선택한 방식이 우선합니다.",
        Kind::Bool
    ),
    field!(
        Review,
        "review.hide_whitespace",
        "공백 변경 숨김",
        "표시만 바뀌며 원본 패치와 스테이징 좌표는 유지합니다.",
        Kind::Bool
    ),
    field!(
        Review,
        "review.advance",
        "검토 완료 후 다음 파일",
        "다음 미검토 파일로 자동 이동합니다.",
        Kind::Bool
    ),
    field!(
        Keyboard,
        "keyboard.bindings",
        "단축키 재정의",
        "JSON: 명령 ID → cmd-alt-f 형태의 키. 아래에서 충돌을 확인합니다.",
        Kind::Json
    ),
    field!(
        Execution,
        "terminal.font_size",
        "터미널 글자 크기",
        "새 세션부터 코드·diff 크기와 독립적으로 적용됩니다. 기본값은 코드 크기입니다.",
        Kind::Number(8, 32)
    ),
    field!(
        Execution,
        "terminal.shell",
        "기본 쉘",
        "실행 파일 경로. 변경은 새 터미널부터 적용됩니다.",
        Kind::Text
    ),
    field!(
        Execution,
        "terminal.args",
        "쉘 시작 인수",
        "JSON 문자열 배열. 기본 [\"-l\",\"-i\"]로 zsh 환경을 읽습니다.",
        Kind::Json
    ),
    field!(
        Execution,
        "tasks.profiles",
        "실행 프로필",
        "JSON 배열: {name,command,cwd,env:{}} · 실행 관리는 작업 화면에서 합니다.",
        Kind::Json
    ),
    field!(
        Execution,
        "logs.auto_scroll",
        "로그 자동 스크롤",
        "새 출력이 추가되면 끝을 따라갑니다.",
        Kind::Bool
    ),
    field!(
        Execution,
        "logs.word_wrap",
        "로그 줄바꿈",
        "긴 로그를 화면 너비에 맞춥니다.",
        Kind::Bool
    ),
    field!(
        Execution,
        "logs.max_lines",
        "로그 보관 줄 수",
        "실행마다 보관할 최대 줄 수입니다.",
        Kind::Number(100, 1000000)
    ),
    field!(
        Execution,
        "spring.jdk",
        "JDK 경로",
        "프로젝트의 JAVA_HOME. 빈 값은 상속한 환경을 사용합니다.",
        Kind::Text
    ),
    field!(
        Execution,
        "spring.gradle",
        "Gradle 경로",
        "빈 값은 확장 런타임의 기본 탐색을 사용합니다.",
        Kind::Text
    ),
    field!(
        Execution,
        "spring.maven",
        "Maven 경로",
        "빈 값은 확장 런타임의 기본 탐색을 사용합니다.",
        Kind::Text
    ),
    field!(
        Execution,
        "spring.profiles",
        "Spring 프로파일",
        "쉼표로 구분한 활성 프로파일입니다.",
        Kind::Text
    ),
    field!(
        Execution,
        "spring.env",
        "환경변수",
        "JSON: {\"NAME\":\"value\"} · 외부 전송 없이 로컬에 저장됩니다.",
        Kind::Json
    ),
    field!(
        Ai,
        "ai.cli",
        "기본 AI CLI",
        "새 세션으로 시작합니다. 실행은 사용자가 직접 선택합니다.",
        Kind::Choice(&[("codex", "Codex"), ("claude", "Claude")])
    ),
    field!(
        Ai,
        "ai.path",
        "실행 파일 경로",
        "빈 값이면 로그인 쉘의 PATH에서 선택한 CLI를 찾습니다.",
        Kind::Text
    ),
    field!(
        Ai,
        "ai.args",
        "추가 인수",
        "JSON 문자열 배열. 인수마다 별도 argv로 전달합니다.",
        Kind::Json
    ),
    field!(
        Ai,
        "ai.start",
        "기본 시작 방식",
        "현재 워크트리의 새 내장 터미널 또는 명령 복사입니다.",
        Kind::Choice(&[("terminal", "내장 터미널"), ("copy", "명령 복사")])
    ),
    field!(
        Extensions,
        "extensions.enabled",
        "프로젝트별 확장 사용",
        "JSON: {\"extension-id\":true} · 앱 기본값을 재정의합니다.",
        Kind::Json
    ),
    field!(
        Extensions,
        "extensions.settings",
        "확장별 설정",
        "JSON: {\"extension-id\":{\"key\":\"value\"}}",
        Kind::Json
    ),
    Field {
        key: "updates.auto_check",
        label: "자동 업데이트 확인",
        hint: "자동 확인만 조절합니다. 설치·재시작은 직접 선택합니다.",
        category: Category::Updates,
        kind: Kind::Bool,
        app_only: true,
    },
];
pub(super) fn default_value(key: &str) -> Value {
    match key {
        "workspace.default_view" => json!("git"),
        "navigation.results_placement" => json!("editor"),
        "navigation.blame_mode" => json!("line"),
        "navigation.symbols_placement" => json!("sidebar"),
        "terminal.shell" => json!("/bin/zsh"),
        "terminal.args" => json!(["-l", "-i"]),
        "ai.cli" => json!("codex"),
        "ai.start" => json!("terminal"),
        "editor.tab_size" => json!(4),
        "terminal.font_size" => json!(13),
        "logs.max_lines" => json!(10000),
        "tasks.profiles" | "ai.args" => json!([]),
        "editor.wrap_patterns"
        | "language.servers"
        | "formatters.config"
        | "keyboard.bindings"
        | "spring.env"
        | "extensions.enabled"
        | "extensions.settings" => json!({}),
        "search.exclude" | "spring.jdk" | "spring.gradle" | "spring.maven" | "spring.profiles"
        | "ai.path" => json!(""),
        "language.auto_signature_help"
        | "editor.word_wrap"
        | "editor.format_on_save"
        | "review.split_diff"
        | "review.hide_whitespace"
        | "review.advance" => json!(false),
        _ => json!(true),
    }
}
fn string_map(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.values().all(Value::is_string))
}
fn string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}
fn commands(value: &Value) -> bool {
    value.as_object().is_some_and(|map| {
        map.values().all(|entry| {
            entry.as_object().is_some_and(|entry| {
                entry
                    .get("path")
                    .is_some_and(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
                    && entry.get("args").is_none_or(string_array)
            })
        })
    })
}
pub(super) fn parse(field: Field, text: &str) -> Result<Value, String> {
    let invalid = || format!("{}: {}", field.label, field.hint);
    let value = match field.kind {
        Kind::Text => json!(text),
        Kind::Number(min, max) => {
            let number = text
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("{min}–{max} 사이 정수를 입력하세요."))?;
            if !(min..=max).contains(&number) {
                return Err(format!("{min}–{max} 사이 정수를 입력하세요."));
            }
            json!(number)
        }
        Kind::Json => {
            serde_json::from_str(text).map_err(|error| format!("JSON을 확인하세요: {error}"))?
        }
        _ => return Err(invalid()),
    };
    let valid = match field.key {
        "terminal.args" | "ai.args" => string_array(&value),
        "language.servers" | "formatters.config" => commands(&value),
        "spring.env" | "keyboard.bindings" => string_map(&value),
        "editor.wrap_patterns" | "extensions.enabled" => value
            .as_object()
            .is_some_and(|map| map.values().all(Value::is_boolean)),
        "extensions.settings" => value
            .as_object()
            .is_some_and(|map| map.values().all(Value::is_object)),
        "tasks.profiles" => value.as_array().is_some_and(|items| {
            let mut names = std::collections::HashSet::new();
            items.iter().all(|item| {
                item.as_object().is_some_and(|item| {
                    item.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.trim().is_empty() && names.insert(s))
                        && item
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.trim().is_empty())
                        && item.get("cwd").is_none_or(Value::is_string)
                        && item.get("env").is_none_or(string_map)
                })
            })
        }),
        "terminal.shell" => !text.trim().is_empty() && !text.contains(['\0', '\n']),
        "ai.path" => !text.contains(['\0', '\n']),
        _ => true,
    };
    if !valid {
        return Err(invalid());
    }
    if matches!(field.key, "terminal.args" | "ai.args")
        && value
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg.as_str().unwrap().contains('\0'))
    {
        return Err("인수에 NUL 문자를 넣을 수 없습니다.".into());
    }
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_schema_default_is_valid() {
        for &field in FIELDS {
            if matches!(field.kind, Kind::Text | Kind::Number(..) | Kind::Json) {
                let value = default_value(field.key);
                let text = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                assert_eq!(parse(field, &text).unwrap(), value, "{}", field.key);
            }
        }
    }
    #[test]
    fn commands_profiles_and_numbers_reject_bad_shapes() {
        for (key, text) in [
            (
                "language.servers",
                r#"{"rust":{"path":"ra","args":"--bad"}}"#,
            ),
            (
                "tasks.profiles",
                r#"[{"name":"x","command":"run"},{"name":"x","command":"run"}]"#,
            ),
            ("editor.tab_size", "0"),
            ("ai.args", r#"[1]"#),
        ] {
            assert!(parse(*FIELDS.iter().find(|f| f.key == key).unwrap(), text).is_err());
        }
    }
}
