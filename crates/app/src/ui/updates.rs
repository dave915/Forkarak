//! One release check and download for the whole window, independent of the open repositories.
use super::*;
use crate::updater::{self, Prepared};

const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default)]
enum Check {
    #[default]
    Idle,
    Running {
        manual: bool,
    },
    Current,
    Failed(String),
}

enum Package {
    Remote,
    Downloading,
    Ready(Prepared),
    Restarting,
}

struct Release {
    tag: String,
    package: Package,
}

#[derive(Default)]
pub(super) struct Updates {
    packaged: bool,
    check: Check,
    release: Option<Release>,
    action_error: Option<String>,
    _timer: Option<Task<()>>,
}

impl Updates {
    pub(super) fn new(cx: &mut Context<Pokarak>) -> Self {
        let packaged = updater::installed_bundle().is_some();
        let timer = packaged.then(|| {
            let executor = cx.background_executor().clone();
            cx.spawn(async move |view, cx| {
                executor.timer(FIRST_CHECK).await;
                loop {
                    if view
                        .update(cx, |app, cx| app.check_update(false, cx))
                        .is_err()
                    {
                        return;
                    }
                    executor.timer(CHECK_INTERVAL).await;
                }
            })
        });
        Self {
            packaged,
            _timer: timer,
            ..Default::default()
        }
    }

    fn checking(&self) -> bool {
        matches!(self.check, Check::Running { .. })
    }

    pub(super) fn restarting(&self) -> bool {
        self.release
            .as_ref()
            .is_some_and(|r| matches!(r.package, Package::Restarting))
    }

    fn downloading(&self) -> bool {
        self.release
            .as_ref()
            .is_some_and(|r| matches!(r.package, Package::Downloading))
    }

    fn finish_check(&mut self, result: Result<Option<String>, String>) {
        let Check::Running { manual } = self.check else {
            return;
        };
        self.check = match result {
            Ok(Some(tag)) => {
                if self.release.as_ref().is_none_or(|r| r.tag != tag) {
                    self.release = Some(Release {
                        tag,
                        package: Package::Remote,
                    });
                    self.action_error = None;
                }
                Check::Idle
            }
            Ok(None) => {
                self.release = None;
                self.action_error = None;
                Check::Current
            }
            Err(error) if manual => Check::Failed(error),
            Err(_) => Check::Idle,
        };
    }
}

impl Pokarak {
    fn check_update(&mut self, manual: bool, cx: &mut Context<Self>) {
        if let Check::Running { manual: show } = &mut self.updates.check {
            *show |= manual;
            return;
        }
        if self.updates.downloading()
            || self.updates.restarting()
            || (!manual
                && self
                    .updates
                    .release
                    .as_ref()
                    .is_some_and(|r| matches!(r.package, Package::Ready(_))))
        {
            return;
        }
        self.updates.check = Check::Running { manual };
        let check = cx.background_spawn(async { updater::check() });
        cx.spawn(async move |view, cx| {
            let result = check.await;
            let _ = view.update(cx, |app, cx| {
                app.updates.finish_check(result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn download_update(&mut self, cx: &mut Context<Self>) {
        if !self.updates.packaged || self.updates.checking() {
            return;
        }
        let Some(release) = &mut self.updates.release else {
            return;
        };
        if !matches!(release.package, Package::Remote) {
            return;
        }
        release.package = Package::Downloading;
        let tag = release.tag.clone();
        self.updates.check = Check::Idle;
        self.updates.action_error = None;
        if self.prefs.dismissed_update.take().is_some() {
            self.prefs.save();
        }
        let task = cx.background_spawn(async move { updater::prepare(&tag) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |app, cx| {
                let Some(release) = &mut app.updates.release else {
                    return;
                };
                release.package = match result {
                    Ok(prepared) => Package::Ready(prepared),
                    Err(error) => {
                        app.updates.action_error =
                            Some(format!("다운로드를 완료하지 못했습니다. {error}"));
                        Package::Remote
                    }
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Release {
            tag,
            package: Package::Ready(_),
        }) = &self.updates.release
        else {
            return;
        };
        #[cfg(target_os = "macos")]
        let terminal = self
            .sessions
            .values()
            .filter_map(|s| s.terminal.as_ref())
            .any(|dock| dock.read(cx).has_running_shell());
        #[cfg(not(target_os = "macos"))]
        let terminal = false;
        let conflict = self.has_unsaved_conflict(cx);
        if !terminal && !conflict {
            self.restart_update(false, window, cx);
            return;
        }
        let tag = tag.clone();
        let view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(entity) = view.upgrade() else {
                return dialog;
            };
            let app = entity.read(cx);
            let restarting = app.updates.restarting();
            let blocked = app.git_is_busy() && !restarting;
            let mut body = div()
                .v_flex()
                .gap_2()
                .child(format!("포카락 {tag}을 설치하고 다시 엽니다."));
            if terminal {
                body = body.child("열려 있는 터미널 세션과 실행 중인 명령이 종료됩니다.");
            }
            if conflict {
                body = body.child("저장하지 않은 충돌 편집 내용이 사라집니다.");
            }
            if blocked {
                body = body.child("진행 중인 Git 작업이 끝나면 재시작할 수 있습니다.");
            }
            let target = view.clone();
            dialog
                .title("업데이트하고 재시작할까요?")
                .width(ui_px(420.).min(window.viewport_size().width - ui_px(24.)))
                .keyboard(!restarting)
                .close_button(!restarting)
                .overlay_closable(!restarting)
                .child(body)
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("update-cancel")
                                .label("취소")
                                .disabled(restarting)
                                .on_click(|_, w, cx| w.close_dialog(cx)),
                        )
                        .child(
                            Button::new("update-confirm")
                                .label(if restarting {
                                    "재시작 준비 중…"
                                } else {
                                    "업데이트하고 재시작"
                                })
                                .disabled(blocked || restarting || app.updates.checking())
                                .on_click(move |_, w, cx| {
                                    let _ = target
                                        .update(cx, |app, cx| app.restart_update(true, w, cx));
                                }),
                        ),
                )
        });
    }

    fn restart_update(&mut self, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy()
            || self.updates.checking()
            || !self
                .updates
                .release
                .as_ref()
                .is_some_and(|r| matches!(r.package, Package::Ready(_)))
        {
            return;
        }
        if let Err(error) = self.remember_update_workspace(cx) {
            self.updates.action_error = Some(error);
            if confirmed {
                window.close_dialog(cx);
            }
            cx.notify();
            return;
        }
        let release = self.updates.release.as_mut().unwrap();
        let Package::Ready(prepared) = std::mem::replace(&mut release.package, Package::Restarting)
        else {
            unreachable!()
        };
        self.updates.action_error = None;
        if !confirmed {
            window.open_dialog(cx, |dialog, _, _| {
                dialog
                    .title("재시작 준비 중…")
                    .width(ui_px(310.))
                    .keyboard(false)
                    .close_button(false)
                    .overlay_closable(false)
            });
        }
        let task = cx.background_spawn(async move { prepared.restart() });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, w, cx| match result {
                Ok(()) => cx.quit(),
                Err(error) => {
                    app.forget_update_workspace();
                    if let Some(r) = &mut app.updates.release {
                        r.package = Package::Remote;
                    }
                    app.updates.action_error =
                        Some(format!("재시작을 준비하지 못했습니다. {error}"));
                    w.close_dialog(cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn update_button(&self, view: WeakEntity<Self>, settings: bool) -> Option<Button> {
        if !self.updates.packaged {
            return None;
        }
        let release = self.updates.release.as_ref()?;
        let (label, ready, mut disabled) = match release.package {
            Package::Remote => ("업데이트 다운로드", false, false),
            Package::Downloading => ("다운로드 중…", false, true),
            Package::Ready(_) => ("업데이트하고 재시작", true, false),
            Package::Restarting => ("재시작 준비 중…", true, true),
        };
        disabled |= self.updates.checking() || (ready && self.git_is_busy());
        Some(
            Button::new(("update-action", usize::from(settings)))
                .small()
                .label(label)
                .disabled(disabled)
                .on_click(move |_, w, cx| {
                    let _ = view.update(cx, |app, cx| {
                        if ready {
                            app.confirm_update(w, cx)
                        } else {
                            app.download_update(cx)
                        }
                    });
                }),
        )
    }

    pub(super) fn update_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let r = self.updates.release.as_ref()?;
        if !self.updates.packaged || self.prefs.dismissed_update.as_ref() == Some(&r.tag) {
            return None;
        }
        let text = if let Some(error) = &self.updates.action_error {
            error.clone()
        } else {
            match r.package {
                Package::Remote => format!("포카락 {} 업데이트를 사용할 수 있습니다.", r.tag),
                Package::Downloading => {
                    format!("포카락 {}을 다운로드하고 검증하는 중입니다.", r.tag)
                }
                Package::Ready(_) if self.git_is_busy() => {
                    "Git 작업이 끝나면 업데이트하고 재시작할 수 있습니다.".into()
                }
                Package::Ready(_) | Package::Restarting => {
                    format!("포카락 {} 설치 준비가 끝났습니다.", r.tag)
                }
            }
        };
        let palette = Palette::new(cx);
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(palette.rgb(LINE))
                .bg(palette.rgb(PANEL))
                .child(div().flex_1().min_w_0().text_ellipsis().child(text))
                .children(self.update_button(cx.weak_entity(), false))
                .child(
                    Button::new("update-dismiss")
                        .small()
                        .ghost()
                        .icon(IconName::X)
                        .accessibility_label("이 버전의 알림 닫기")
                        .tooltip("이 버전의 알림 닫기")
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.prefs.dismissed_update =
                                app.updates.release.as_ref().map(|r| r.tag.clone());
                            app.prefs.save();
                            cx.notify();
                        })),
                ),
        )
    }

    pub(super) fn update_settings(&self, view: WeakEntity<Self>, cx: &App) -> Div {
        let palette = Palette::new(cx);
        let version = format!("현재 v{}", env!("CARGO_PKG_VERSION"));
        let versions = self.updates.release.as_ref().map_or_else(
            || version.clone(),
            |r| format!("{version}   ·   새 버전 {}", r.tag),
        );
        let message = if let Check::Failed(error) = &self.updates.check {
            Some(format!("업데이트를 확인하지 못했습니다. {error}"))
        } else if self.updates.checking() {
            Some("새 버전 확인 중…".into())
        } else if let Some(error) = &self.updates.action_error {
            Some(error.clone())
        } else if let Some(r) = &self.updates.release {
            if !self.updates.packaged {
                Some("설치는 배포용 Forkarak.app에서 사용할 수 있습니다.".into())
            } else if matches!(r.package, Package::Ready(_)) && self.git_is_busy() {
                Some("Git 작업이 끝나면 재시작할 수 있습니다.".into())
            } else {
                None
            }
        } else if matches!(self.updates.check, Check::Current) {
            Some("최신 버전을 사용 중입니다.".into())
        } else {
            None
        };
        let check = view.clone();
        div()
            .v_flex()
            .gap_1()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .pt_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().text_ellipsis().child(versions))
                    .child(
                        Button::new("check-update")
                            .small()
                            .label("업데이트 확인")
                            .disabled(
                                self.updates.checking()
                                    || self.updates.downloading()
                                    || self.updates.restarting(),
                            )
                            .on_click(move |_, _, cx| {
                                let _ = check.update(cx, |app, cx| app.check_update(true, cx));
                            }),
                    ),
            )
            .when_some(message, |row, text| {
                row.child(
                    div()
                        .text_size(ui_text(12.))
                        .text_color(palette.rgb(MUTED))
                        .child(text),
                )
            })
            .when_some(self.update_button(view, true), |row, button| {
                row.child(div().child(button))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Check, Updates};

    #[test]
    fn failed_check_keeps_known_release_and_only_reports_manual_failures() {
        let mut state = Updates {
            check: Check::Running { manual: false },
            ..Default::default()
        };
        state.finish_check(Ok(Some("v1.2.0".into())));
        state.check = Check::Running { manual: false };
        state.finish_check(Err("offline".into()));
        assert!(matches!(state.check, Check::Idle));
        assert_eq!(state.release.as_ref().unwrap().tag, "v1.2.0");
        state.check = Check::Running { manual: true };
        state.finish_check(Err("still offline".into()));
        assert!(matches!(&state.check, Check::Failed(error) if error == "still offline"));
        assert_eq!(state.release.as_ref().unwrap().tag, "v1.2.0");
        state.check = Check::Running { manual: true };
        state.finish_check(Ok(None));
        assert!(state.release.is_none());
        assert!(matches!(state.check, Check::Current));
    }
}
