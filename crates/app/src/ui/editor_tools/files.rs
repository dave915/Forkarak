//! Explicit forms call only the guarded core operations; no UI filesystem writes.
use super::*;
use pokarak_core::operations::files as operations;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    File,
    Directory,
    Move,
}
struct FileForm {
    owner: WeakEntity<Pokarak>,
    root: PathBuf,
    mode: Mode,
    source: Entity<InputState>,
    destination: Entity<InputState>,
    identity: Option<operations::PathIdentity>,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}
impl FileForm {
    fn new(
        owner: WeakEntity<Pokarak>,
        root: PathBuf,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(path.to_string_lossy().to_string())
                .placeholder("현재 경로 (워크트리 기준)")
        });
        let destination =
            cx.new(|cx| InputState::new(window, cx).placeholder("새 경로 (기존 상위 폴더 안)"));
        let subscription = cx.subscribe(&source, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.identity = None;
                this.error = None;
                cx.notify();
            }
        });
        Self {
            owner,
            root,
            mode: Mode::File,
            source,
            destination,
            identity: None,
            busy: false,
            error: None,
            _subscriptions: vec![subscription],
        }
    }
    fn inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let root = self.root.clone();
        let path = PathBuf::from(self.source.read(cx).value().as_ref());
        let submitted = path.clone();
        let task = cx.background_spawn(async move {
            operations::inspect_path(&root, &path).map_err(|e| format!("{e:#}"))
        });
        cx.spawn_in(window, async move |form, cx| {
            let result = task.await;
            let _ = form.update_in(cx, |form, _, cx| {
                form.busy = false;
                if Path::new(form.source.read(cx).value().as_ref()) != submitted {
                    return;
                }
                match result {
                    Ok(identity) => form.identity = Some(identity),
                    Err(error) => form.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn execute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let destination = PathBuf::from(self.destination.read(cx).value().as_ref());
        if destination.as_os_str().is_empty() {
            self.error = Some("새 경로를 입력하세요.".into());
            cx.notify();
            return;
        }
        let identity = self.identity.clone();
        let mode = self.mode;
        let root = self.root.clone();
        if mode == Mode::Move && identity.is_none() {
            self.error = Some("원본 확인을 먼저 실행하세요.".into());
            cx.notify();
            return;
        }
        let permitted=self.owner.update(cx,|app,cx|{
            app.capture_code_buffers(cx);
            if app.current.as_ref()!=Some(&root) || app.opening_path.is_some() || app.git_is_busy() || app.loading || app.file_loading{return Err("현재 워크트리가 변경되었거나 다른 작업이 실행 중입니다.".to_string());}
            if let Some(identity)=&identity && (app.code_edits.has_draft_under(&root,&identity.path) || app.code_edits.has_draft_under(&root,&destination)) {
                return Err("이동 경로에 저장하지 않은 초안이 있습니다. 먼저 저장하거나 디스크와 조정하세요. 초안은 유지됩니다.".into());
            }
            Ok(())
        });
        match permitted {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
            Err(_) => return,
        }
        self.busy = true;
        self.error = None;
        let target = destination.clone();
        let source = identity.as_ref().map(|i| i.path.clone());
        let task = cx.background_spawn(async move {
            match mode {
                Mode::File => operations::create_file(&root, &target, b""),
                Mode::Directory => operations::create_directory(&root, &target),
                Mode::Move => operations::move_path(&root, identity.as_ref().unwrap(), &target),
            }
            .map_err(|e| format!("{e:#}"))
        });
        let root = self.root.clone();
        cx.spawn_in(window, async move |form, cx| {
            let result = task.await;
            let _ = form.update_in(cx, |form, window, cx| {
                form.busy = false;
                match result {
                    Err(error) => {
                        form.error = Some(error);
                        form.identity = None;
                    }
                    Ok(()) => {
                        let _ = form.owner.update(cx, |app, cx| {
                            if let Some(source) = source {
                                app.remap_clean_editor_paths(&root, &source, &destination);
                            }
                            if app.current.as_ref() == Some(&root) && app.opening_path.is_none() {
                                app.status = format!(
                                    "{} {}",
                                    destination.display(),
                                    if mode == Mode::Move {
                                        "이동됨"
                                    } else {
                                        "생성됨"
                                    }
                                );
                                window.close_dialog(cx);
                                app.refresh(window, cx);
                                if mode == Mode::File {
                                    app.open_file(destination.clone(), true, None, window, cx);
                                    app.toggle_code(true, cx);
                                }
                            }
                            cx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
impl Render for FileForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut modes = div().flex().gap_2();
        for (id, label, mode) in [
            ("new-file", "파일 생성", Mode::File),
            ("new-folder", "폴더 생성", Mode::Directory),
            ("move-path", "이름 변경 / 이동", Mode::Move),
        ] {
            modes = modes.child(
                Button::new(id)
                    .small()
                    .ghost()
                    .selected(self.mode == mode)
                    .disabled(self.busy)
                    .label(label)
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.mode = mode;
                        s.identity = None;
                        s.error = None;
                        cx.notify();
                    })),
            );
        }
        div().v_flex().gap_3().child(div().text_size(ui_text(11.)).child(format!("대상 워크트리: {}",self.root.display())))
            .child(modes)
            .when(self.mode==Mode::Move,|body|body.child(Input::new(&self.source).disabled(self.busy)).child(Button::new("inspect-move-source").small().label("원본 확인").disabled(self.busy).on_click(cx.listener(|s,_,w,cx|s.inspect(w,cx)))))
            .children(self.identity.as_ref().map(|i|div().text_size(ui_text(11.)).child(format!("확인됨: {} {}",if i.is_directory{"폴더"}else{"파일"},i.path.display()))))
            .child(Input::new(&self.destination).disabled(self.busy))
            .child(div().text_size(ui_text(11.)).child("워크트리 안의 상대 경로를 입력하세요. 기존 대상은 덮어쓰지 않습니다. 폴더 이동은 하위 항목을 포함하며 저장되지 않은 초안이 있으면 차단합니다."))
            .children(self.error.as_ref().map(|e|div().text_color(rgb(RED)).child(e.clone())))
            .child(div().flex().justify_end().gap_2()
                .child(Button::new("cancel-file-form").ghost().label("닫기").disabled(self.busy).on_click(|_,w,cx|w.close_dialog(cx)))
                .child(Button::new("execute-file-form").label(if self.busy{"실행 중…"}else if self.mode==Mode::Move{"이동 실행"}else{"생성 실행"}).disabled(self.busy || (self.mode==Mode::Move && self.identity.is_none())).on_click(cx.listener(|s,_,w,cx|s.execute(w,cx)))))
    }
}
impl Pokarak {
    pub(in crate::ui) fn open_file_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) || self.git_is_busy() || self.loading {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        let path = self.selected().unwrap_or_default();
        let owner = cx.entity().downgrade();
        let form = cx.new(|cx| FileForm::new(owner, root, path, window, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("파일 / 폴더 관리")
                .width(ui_px(620.))
                .keyboard(false)
                .close_button(false)
                .overlay_closable(false)
                .child(form.clone())
        });
    }
    fn remap_clean_editor_paths(&mut self, root: &Path, source: &Path, destination: &Path) {
        fn remap(path: &Path, source: &Path, destination: &Path) -> PathBuf {
            path.strip_prefix(source)
                .map(|suffix| destination.join(suffix))
                .unwrap_or_else(|_| path.to_owned())
        }
        self.code_edits.forget_clean_under(root, source);
        if let Some(preview) = self.code_edits.tools.preview.get_mut(root) {
            *preview = remap(preview, source, destination);
        }
        if let Some(session) = self.sessions.get_mut(root) {
            session.saved.tabs = session
                .saved
                .tabs
                .iter()
                .map(|p| remap(p, source, destination))
                .collect();
            session.saved.selected = session
                .saved
                .selected
                .as_ref()
                .map(|p| remap(p, source, destination));
            session.saved.positions = std::mem::take(&mut session.saved.positions)
                .into_iter()
                .map(|(p, v)| (remap(&p, source, destination), v))
                .collect();
            session.documents.retain(|p, _| !p.starts_with(source));
            session.diffs.retain(|p, _| !p.starts_with(source));
            for point in session.history.iter_mut().chain(session.forward.iter_mut()) {
                point.path = remap(&point.path, source, destination);
            }
        }
    }
}
