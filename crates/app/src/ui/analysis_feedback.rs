//! Compact feedback and a single, shared bottom Output/results space.
use super::*;

#[derive(Default)]
pub(super) struct AnalysisFeedback {
    root: Option<PathBuf>,
    observed_error: Option<String>,
    output: String,
    pub output_open: bool,
    retry: Option<RetryNavigation>,
}

#[derive(Clone)]
struct RetryNavigation {
    root: PathBuf,
    path: PathBuf,
    kind: NavigationKind,
    position: Position,
    error: String,
}

impl AnalysisFeedback {
    fn observe(&mut self, root: &Option<PathBuf>, error: &Option<String>) {
        if &self.root != root {
            *self = Self {
                root: root.clone(),
                ..Default::default()
            };
        }
        if self.observed_error != *error {
            if let Some(error) = error {
                self.append(error);
            }
            self.observed_error = error.clone();
        }
    }

    pub fn append(&mut self, raw: &str) {
        if raw.trim().is_empty() {
            return;
        }
        if !self.output.is_empty() {
            self.output.push('\n');
        }
        self.output.push_str(raw);
        // Keep a UTF-8-safe bounded tail. Rendering is virtualized independently.
        const LIMIT: usize = 128 * 1024;
        if self.output.len() > LIMIT {
            let mut start = self.output.len() - LIMIT;
            while !self.output.is_char_boundary(start) {
                start += 1;
            }
            self.output.drain(..start);
        }
    }
}

/// Never expose protocol JSON or multiline stderr in the summary row.
fn error_summary(raw: &str) -> String {
    let text = raw.to_lowercase();
    let summary = if text.contains("nobuildsystemfound") {
        "Maven/Gradle 프로젝트 설정이 없습니다. 소스 상위 폴더의 pom.xml 또는 Gradle 설정을 확인하세요."
    } else if [
        "unsupportedclassversionerror",
        "class file version",
        "unable to locate a java runtime",
        "could not create the java virtual machine",
        "java_home is not set",
        "java_home is set to an invalid directory",
        "no java runtime present",
    ]
    .iter()
    .any(|evidence| text.contains(evidence))
    {
        "언어 서버의 Java 실행 환경을 확인하세요. 서버가 요구하는 JDK와 실행 경로를 설정한 뒤 다시 시도하세요."
    } else if text.contains("allowprojectimports") {
        "프로젝트 분석에는 신뢰 확인이 필요합니다. 분석 허용 후 다시 시도하세요."
    } else if text.contains("project import") || text.contains("workspaceimport") {
        "프로젝트를 분석에 가져오지 못했습니다. Output에서 빌드 설정·의존성 오류를 확인하세요."
    } else if text.contains("language server unavailable")
        || text.contains("no language server configured")
    {
        "언어 서버를 찾을 수 없습니다. 설치 경로와 언어 서버 설정을 확인하세요."
    } else if text.contains("timed out") || text.contains("timeout") {
        "응답 시간이 초과되었습니다. Output을 확인하고 다시 시도하세요."
    } else {
        let first = raw
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        if first.contains(['{', '}']) || first.is_empty() {
            "요청을 완료하지 못했습니다. Output에서 상세 내용을 확인하세요."
        } else {
            first
        }
    };
    let mut compact: String = summary.chars().take(120).collect();
    if summary.chars().count() > 120 {
        compact.push('…');
    }
    compact
}

impl Pokarak {
    pub(super) fn observe_feedback(&mut self) {
        self.analysis_feedback.observe(&self.current, &self.error);
    }

    pub(super) fn record_analysis_failure(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        kind: NavigationKind,
        position: Position,
        error: String,
    ) {
        self.analysis_feedback.retry = Some(RetryNavigation {
            root,
            path,
            kind,
            position,
            error,
        });
    }

    pub(super) fn render_error_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let error = self.error.as_deref().unwrap_or_default();
        let retry = self
            .analysis_feedback
            .retry
            .as_ref()
            .filter(|retry| {
                self.current.as_ref() == Some(&retry.root)
                    && self.selected().as_ref() == Some(&retry.path)
                    && retry.error == error
                    && self.session().is_some_and(|s| s.saved.code)
            })
            .cloned();
        div()
            .h(px(32.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .bg(palette.rgb(SIDE))
            .text_size(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .text_color(palette.rgb(RED))
                    .child(error_summary(error)),
            )
            .child(
                Button::new("error-details")
                    .label("Details")
                    .tooltip("Output에서 원문 보기")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.analysis_feedback.output_open = true;
                        cx.notify();
                    })),
            )
            .when_some(retry, |row, retry| {
                row.child(
                    Button::new("retry-analysis")
                        .label("다시 시도")
                        .ghost()
                        .small()
                        .disabled(self.navigating)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.error = None;
                            this.request_navigation(
                                retry.kind.clone(),
                                Some(retry.position),
                                window,
                                cx,
                            );
                        })),
                )
            })
            .child(
                Button::new("dismiss-error")
                    .icon(IconName::X)
                    .tooltip("알림 닫기")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.error = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn output_button(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id)
            .label("Output")
            .ghost()
            .small()
            .selected(self.analysis_feedback.output_open)
            .disabled(self.analysis_feedback.output.is_empty())
            .on_click(cx.listener(|this, _, _, cx| {
                this.analysis_feedback.output_open = !this.analysis_feedback.output_open;
                cx.notify();
            }))
    }

    pub(super) fn render_output(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let lines = Arc::new(
            self.analysis_feedback
                .output
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        );
        let widest = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, line)| line.chars().count())
            .map(|(i, _)| i)
            .unwrap_or(0);
        let output = self.analysis_feedback.output.clone();
        div()
            .id("analysis-output")
            .h(px(210.))
            .max_h(relative(0.3))
            .min_h_0()
            .flex_none()
            .v_flex()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .bg(palette.rgb(SIDE))
            .overflow_hidden()
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child("Output")
                    .when(self.results_open, |row| {
                        row.child(
                            Button::new("output-results")
                                .label("검색 / 사용처")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.analysis_feedback.output_open = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("copy-output")
                            .label("모두 복사")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(output.clone()))
                            }),
                    )
                    .child(
                        Button::new("close-output")
                            .icon(IconName::X)
                            .tooltip("하단 패널 접기")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.analysis_feedback.output_open = false;
                                this.results_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                uniform_list("output-lines", lines.len(), move |range, _, _| {
                    range
                        .map(|index| {
                            div()
                                .h(px(19.))
                                .px_3()
                                .font_family("JetBrains Mono")
                                .text_size(px(11.))
                                .text_color(palette.rgb(MUTED))
                                .whitespace_nowrap()
                                .child(lines[index].clone())
                                .into_any_element()
                        })
                        .collect()
                })
                .with_width_from_item(Some(widest))
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{AnalysisFeedback, error_summary};
    use std::path::PathBuf;
    #[test]
    fn summaries_explain_build_metadata_and_require_runtime_evidence() {
        let raw = "Project import failed: {\"error\":\"noBuildSystemFound\"}\nServer stderr:\nJava 17 source\n";
        assert!(error_summary(raw).contains("Maven/Gradle"));
        assert!(!error_summary(raw).contains("JDK"));
        assert!(!error_summary("Project import failed: dependencies").contains("JDK"));
        assert!(error_summary("UnsupportedClassVersionError").contains("JDK"));
        assert_eq!(
            error_summary("git fetch failed\nsecret multiline diagnostic"),
            "git fetch failed"
        );
        assert!(!error_summary("{\"message\":\"raw json\"}").contains('{'));
        assert!(error_summary(&"한".repeat(5000)).chars().count() <= 121);
    }
    #[test]
    fn errors_are_retained_after_dismissal_and_isolated_by_worktree() {
        let mut feedback = AnalysisFeedback::default();
        let root = Some(PathBuf::from("/fixture/a"));
        let error = Some("raw diagnostic\nline two".to_owned());
        feedback.observe(&root, &error);
        feedback.observe(&root, &error);
        assert_eq!(feedback.output, error.clone().unwrap());
        feedback.observe(&root, &None);
        assert_eq!(feedback.output, error.unwrap());
        feedback.observe(&Some(PathBuf::from("/fixture/b")), &None);
        assert!(feedback.output.is_empty());
        feedback.append(&"한".repeat(100000));
        assert!(feedback.output.len() <= 128 * 1024);
    }
}
