use super::*;
use crate::ui::dialogs::DialogWindowExt as _;

impl Pokarak {
    pub(in crate::ui) fn language_manager_settings(&self, view: WeakEntity<Self>, cx: &App) -> Div {
        let palette = Palette::new(cx);
        div().v_flex().gap_1().py_2()
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("언어 지원"))
                .child(Button::new("manage-language-support").small().label("언어 서버 선택·설치…")
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = view.update(cx, |app, cx| app.open_language_manager(window, cx));
                    })))
            .child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED))
                .child("사용할 언어를 켜고 필요한 서버를 설치하세요. 워크트리에서 분석을 허용하면 해당 서버로 코드 탐색과 자동완성을 사용합니다."))
    }
    pub(in crate::ui) fn open_language_manager(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.language_manager.refresh();
        let view = cx.weak_entity();
        window.open_app_dialog(cx, move |dialog, window, cx| {
            let Some(entity) = view.upgrade() else { return dialog; };
            let app = entity.read(cx);
            let palette = Palette::new(cx);
            let manager = &app.language_manager;
            let mut body = div().id("language-manager-scroll").v_flex().gap_2()
                .max_h((window.viewport_size().height - ui_px(250.)).max(ui_px(180.)))
                .overflow_y_scroll()
                .child(div().text_size(ui_text(12.)).child("공식 배포 도구를 앱 전용 폴더에 준비합니다. 설치만으로 언어 서버·프로젝트 빌드·가져오기를 실행하지 않습니다."))
                .child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED))
                    .child("설치·업데이트는 앱 전체에 적용됩니다. 분석 사용 여부는 현재 프로젝트 설정을 따릅니다. 업데이트는 직접 선택하며 실패·취소 시 기존 버전을 유지합니다."));
            body = body.child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child(app.current_project.as_ref().map(|p| format!("분석 설정 대상 프로젝트: {}", p.display())).unwrap_or_else(|| "분석 설정 대상: 앱 기본값".into())));
            if let Ok(tools) = &manager.tools {
                let root = tools.root().to_path_buf();
                body = body.child(div().flex().items_center().gap_2()
                    .child(div().flex_1().min_w_0().text_size(ui_text(11.)).child(format!("저장 위치: {}", root.display())))
                    .child(Button::new("reveal-language-tools").small().ghost().label("Finder에서 보기")
                        .disabled(!root.exists()).on_click(move |_, _, cx| cx.reveal_path(&root))));
            }
            if let Some(error) = manager.error.as_ref().or_else(|| manager.tools.as_ref().err()) {
                body = body.child(div().text_size(ui_text(12.)).text_color(palette.rgb(RED)).child(error.clone()));
            }
            if let Some(pending) = &manager.pending {
                let name = manager.packages.iter().find(|p| p.id == pending.package).map(|p| p.name.as_str()).unwrap_or(&pending.package);
                body = body.child(div().p_2().bg(palette.rgb(0x293848))
                    .child(format!("{name} {} ‘원래 요청 계속’을 선택하면 원래 파일·선택·분석 허용을 다시 확인합니다.", if manager.ready(&pending.package) && !manager.running.contains_key(&pending.package) { "준비가 완료되었습니다." } else { "준비가 필요합니다. 설치 후" })));
            }
            let mut packages = manager.packages.iter().collect::<Vec<_>>();
            packages.sort_by_key(|p| (!manager.pending.as_ref().is_some_and(|r| r.package == p.id), p.name.clone()));
            for package in packages { body = body.child(app.language_package_row(package, &view, cx)); }
            let text_only = language_catalog().into_iter().filter(|l| l.package.is_none()).map(|l| format!("{}: {}", l.name, l.limitations.join(" "))).collect::<Vec<_>>();
            if !text_only.is_empty() {
                body = body.child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child(format!("보기·텍스트 검색 지원 · {}", text_only.join(" · "))));
            }
            let pending_ready = manager.pending.as_ref().is_some_and(|p| manager.ready(&p.package) && !manager.running.contains_key(&p.package));
            let continue_view = view.clone();
            let refresh_view = view.clone();
            let settings_view = view.clone();
            let close_view = view.clone();
            let dismiss_view = view.clone();
            dialog.title("언어 지원 관리").width(ui_px(800.).min(window.viewport_size().width - ui_px(24.)))
                .on_close(move |_, _, cx| { let _ = dismiss_view.update(cx, |app, _| app.cancel_managed_language_request()); })
                .child(body)
                .footer(div().flex().items_center().gap_2()
                    .child(Button::new("language-manager-refresh").small().ghost().label("상태 다시 확인")
                        .on_click(move |_, _, cx| { let _ = refresh_view.update(cx, |app, cx| { app.language_manager.refresh(); cx.notify(); }); }))
                    .child(Button::new("language-manager-settings").small().ghost().label("언어 설정…")
                        .on_click(move |_, window, cx| { window.close_dialog(cx); let _ = settings_view.update(cx, |app, cx| app.open_settings_section("language", window, cx)); }))
                    .child(div().flex_1())
                    .when(manager.pending.is_some(), |row| row.child(Button::new("language-manager-continue").small().label("원래 요청 계속").disabled(!pending_ready)
                        .on_click(move |_, window, cx| { let _ = continue_view.update(cx, |app, cx| app.continue_prepared_language(window, cx)); })))
                    .child(Button::new("language-manager-close").small().label("닫기")
                        .on_click(move |_, window, cx| { let _ = close_view.update(cx, |app, _| app.cancel_managed_language_request()); window.close_dialog(cx); })))
        });
        cx.notify();
    }
    fn language_package_row(
        &self,
        package: &ToolPackage,
        view: &WeakEntity<Self>,
        cx: &App,
    ) -> Div {
        let manager = &self.language_manager;
        let palette = Palette::new(cx);
        let status = manager.statuses.get(&package.id);
        let installed = status.and_then(|s| s.installed.as_ref());
        let running = manager.running.contains_key(&package.id);
        let progress = manager.progress(&package.id);
        let expanded = manager.expanded.contains(&package.id);
        let retry = progress
            .as_ref()
            .is_some_and(|p| matches!(p.phase, InstallPhase::Failed | InstallPhase::Cancelled));
        let distribution = match installed
            .map(|t| &t.distribution)
            .unwrap_or(&package.distribution)
        {
            ToolDistribution::Bundled => "앱에 포함됨",
            ToolDistribution::FirstUseDownload if installed.is_some() => "앱에서 다운로드함",
            ToolDistribution::FirstUseDownload => "첫 사용 시 다운로드",
        };
        let summary = match installed {
            Some(tool) => format!(
                "{distribution} · {}{}",
                tool.version,
                if status.is_some_and(|s| s.problem.is_some()) {
                    " · 복구 필요"
                } else {
                    " · 준비됨"
                }
            ),
            None if manager.status_failed => format!("{distribution} · 상태 확인 필요"),
            None => format!("{distribution} · 미설치"),
        };
        let (details_view, id) = (view.clone(), package.id.clone());
        let mut actions = div().flex().items_center().gap_2().child(
            Button::new(SharedString::from(format!(
                "language-details-{}",
                package.id
            )))
            .small()
            .ghost()
            .label(if expanded {
                "정보 접기"
            } else {
                "버전·라이선스"
            })
            .selected(expanded)
            .on_click(move |_, _, cx| {
                let _ = details_view.update(cx, |app, cx| {
                    if !app.language_manager.expanded.remove(&id) {
                        app.language_manager.expanded.insert(id.clone());
                    }
                    cx.notify();
                });
            }),
        );
        if running {
            let (view, id) = (view.clone(), package.id.clone());
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "language-cancel-{}",
                    package.id
                )))
                .small()
                .label("취소")
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |app, cx| app.cancel_language_install(&id, cx));
                }),
            );
        } else {
            let (view, id) = (view.clone(), package.id.clone());
            let update = installed.is_some();
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "language-install-{}",
                    package.id
                )))
                .small()
                .label(if retry {
                    "다시 시도"
                } else if status.is_some_and(|s| s.problem.is_some()) {
                    "복구 다운로드"
                } else if update {
                    "업데이트 다운로드"
                } else {
                    "설치"
                })
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |app, cx| {
                        app.begin_language_install(id.clone(), update, window, cx)
                    });
                }),
            );
        }
        let mut row = div()
            .v_flex()
            .gap_1()
            .py_2()
            .border_b_1()
            .border_color(palette.rgb(LINE))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(ui_px(180.))
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(package.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(ui_text(11.))
                                    .text_color(palette.rgb(MUTED))
                                    .child(summary),
                            ),
                    )
                    .child(actions),
            );
        let mut toggles = div().flex().flex_wrap().gap_1();
        for language in &package.languages {
            let enabled = self.managed_language_enabled(language);
            let (view, language) = (view.clone(), language.clone());
            toggles = toggles.child(
                Button::new(SharedString::from(format!("language-enable-{language}")))
                    .small()
                    .ghost()
                    .label(format!(
                        "{language} · {}",
                        if enabled { "사용" } else { "끔" }
                    ))
                    .selected(enabled)
                    .on_click(move |_, _, cx| {
                        let _ = view.update(cx, |app, cx| {
                            app.set_managed_language_enabled(&language, !enabled, cx)
                        });
                    }),
            );
        }
        row = row.child(toggles);
        if package.languages.iter().any(|language| {
            custom_override(
                self.prefs
                    .option(self.current_project.as_deref(), "language.servers"),
                language,
            ) || legacy_override(language)
        }) {
            row = row.child(div().text_size(ui_text(11.)).text_color(palette.rgb(BLUE)).child("사용자 지정 서버 경로가 우선합니다. 관리 도구를 설치해도 기존 경로를 바꾸지 않습니다."));
        }
        if let Some(problem) = status.and_then(|s| s.problem.as_ref()) {
            row = row.child(
                div()
                    .text_size(ui_text(11.))
                    .text_color(palette.rgb(RED))
                    .child(problem.clone()),
            );
        }
        if let Some(progress) = &progress {
            row = row.child(
                div()
                    .text_size(ui_text(11.))
                    .text_color(
                        palette.rgb(if matches!(progress.phase, InstallPhase::Failed) {
                            RED
                        } else {
                            MUTED
                        }),
                    )
                    .child(progress_line(progress)),
            );
            if progress.phase == InstallPhase::Failed
                && let Some(detail) = progress.error.clone()
            {
                row = row.child(
                    Button::new(SharedString::from(format!(
                        "copy-install-error-{}",
                        package.id
                    )))
                    .small()
                    .ghost()
                    .icon(IconName::Copy)
                    .label("세부 정보 복사")
                    .accessibility_label("설치 오류 세부 정보 복사")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(detail.clone()));
                    }),
                );
            }
            if matches!(
                progress.phase,
                InstallPhase::Failed | InstallPhase::Cancelled
            ) && installed.is_some()
            {
                row = row.child(
                    div()
                        .text_size(ui_text(11.))
                        .child("기존 활성 버전이 유지됩니다."),
                );
            }
        }
        // Limits are visible before installation, not hidden behind an approval checkbox.
        for limitation in package
            .prerequisites
            .iter()
            .chain(package.limitations.iter())
        {
            row = row.child(
                div()
                    .text_size(ui_text(11.))
                    .text_color(palette.rgb(MUTED))
                    .child(limitation.clone()),
            );
        }
        if expanded {
            let source = package.source_url.clone();
            row = row
                .child(
                    div()
                        .text_size(ui_text(11.))
                        .child(format!("라이선스: {}", package.license)),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "language-source-{}",
                        package.id
                    )))
                    .small()
                    .ghost()
                    .label("공식 프로젝트·소스 보기")
                    .on_click(move |_, _, cx| cx.open_url(&source)),
                );
            if let Some(tool) = installed {
                for (index, artifact) in tool.artifacts.iter().enumerate() {
                    let url = artifact.source_url.clone();
                    row = row
                        .child(div().text_size(ui_text(11.)).child(format!(
                            "구성 요소 {} · 버전 {} · 체크섬 {}",
                            index + 1,
                            artifact.version,
                            artifact.checksum
                        )))
                        .child(
                            Button::new(SharedString::from(format!(
                                "language-artifact-{}-{index}",
                                package.id
                            )))
                            .small()
                            .ghost()
                            .label("이 구성 요소의 공식 다운로드 원본")
                            .tooltip(artifact.source_url.clone())
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                        );
                }
                for (index, notice) in tool.notices.iter().enumerate() {
                    let path = PathBuf::from(notice);
                    row = row.child(div().text_size(ui_text(11.)).child(notice.clone()));
                    if path.is_absolute() && path.is_file() {
                        row = row.child(
                            Button::new(SharedString::from(format!(
                                "language-notice-{}-{index}",
                                package.id
                            )))
                            .small()
                            .ghost()
                            .label("설치된 라이선스·고지 위치 보기")
                            .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        );
                    }
                }
            } else {
                row = row.child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child("설치 후 실제 구성 요소 버전·원본·체크섬·고지를 표시합니다. 서버가 제공하는 기능은 분석 시작 시 협상한 결과를 따릅니다."));
            }
        }
        row
    }
}
