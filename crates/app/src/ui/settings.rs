use super::*;
use crate::theme::DisplaySettings;
use crate::ui::dialogs::DialogWindowExt as _;

#[derive(Clone, Copy)]
enum Setting {
    InterfaceFont,
    CodeFont,
    Zoom,
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Self::InterfaceFont => "인터페이스 글자 크기",
            Self::CodeFont => "코드·Diff 글자 크기",
            Self::Zoom => "화면 배율",
        }
    }

    fn value(self, display: DisplaySettings) -> String {
        match self {
            Self::InterfaceFont => format!("{} pt", display.ui_font_size),
            Self::CodeFont => format!("{} pt", display.code_font_size),
            Self::Zoom => format!("{}%", display.ui_zoom_percent),
        }
    }

    fn button_ids(self) -> [&'static str; 2] {
        match self {
            Self::InterfaceFont => ["ui-font-smaller", "ui-font-larger"],
            Self::CodeFont => ["font-smaller", "font-larger"],
            Self::Zoom => ["ui-zoom-out", "ui-zoom-in"],
        }
    }

    fn step(self, mut display: DisplaySettings, direction: i16) -> DisplaySettings {
        match self {
            Self::InterfaceFont => {
                display.ui_font_size = (display.ui_font_size + f32::from(direction)).clamp(
                    *DisplaySettings::UI_RANGE.start(),
                    *DisplaySettings::UI_RANGE.end(),
                )
            }
            Self::CodeFont => {
                display.code_font_size = (display.code_font_size + f32::from(direction)).clamp(
                    *DisplaySettings::CODE_RANGE.start(),
                    *DisplaySettings::CODE_RANGE.end(),
                )
            }
            Self::Zoom => {
                display.ui_zoom_percent = display
                    .ui_zoom_percent
                    .saturating_add_signed(direction * 10)
                    .clamp(
                        *DisplaySettings::ZOOM_RANGE.start(),
                        *DisplaySettings::ZOOM_RANGE.end(),
                    )
            }
        }
        display
    }

    fn row(self, display: DisplaySettings, view: &WeakEntity<Pokarak>) -> Div {
        let button = |index: usize, direction: i16, icon, action: &'static str| {
            let view = view.clone();
            Button::new(self.button_ids()[index])
                .icon(icon)
                .small()
                .accessibility_label(format!("{} {action}", self.label()))
                .tooltip(format!("{} {action}", self.label()))
                .disabled(self.step(display, direction) == display)
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |app, cx| {
                        app.set_display(self.step(app.prefs.display, direction), cx);
                    });
                })
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.label()),
            )
            .child(button(0, -1, IconName::Minus, "줄이기"))
            .child(
                div()
                    .min_w(ui_text(48.))
                    .text_center()
                    .child(self.value(display)),
            )
            .child(button(1, 1, IconName::Plus, "늘리기"))
    }
}

#[path = "settings_schema.rs"]
mod schema;
use gpui_kit::component::input::{Textarea, TextareaState};
use schema::{Category, FIELDS, Field, Kind, default_value};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

type DialogState = Rc<RefCell<SettingsDialog>>;
struct SettingsDialog {
    category: Category,
    root: Option<PathBuf>,
    project: Option<PathBuf>,
    worktree: Option<PathBuf>,
    inputs: HashMap<&'static str, FieldInput>,
    keyboard_filter: Entity<InputState>,
    feedback: String,
    command_details: bool,
    copied: Option<&'static str>,
}
#[derive(Clone)]
enum FieldInput {
    Line(Entity<InputState>),
    Multi(Entity<TextareaState>),
}
impl FieldInput {
    fn value(&self, cx: &App) -> String {
        match self {
            Self::Line(input) => input.read(cx).value().to_string(),
            Self::Multi(input) => input.read(cx).value().to_string(),
        }
    }
    fn element(&self, id: SharedString, disabled: bool) -> AnyElement {
        match self {
            Self::Line(input) => Input::new(input)
                .id(id)
                .disabled(disabled)
                .into_any_element(),
            Self::Multi(input) => div()
                .id(id)
                .child(Textarea::new(input).disabled(disabled))
                .into_any_element(),
        }
    }
}
impl SettingsDialog {
    fn load_inputs(&mut self, prefs: &Preferences, window: &mut Window, cx: &mut App) {
        self.inputs.clear();
        for &field in FIELDS {
            if !matches!(field.kind, Kind::Text | Kind::Json | Kind::Number(..)) {
                continue;
            }
            let value = effective_value(prefs, self.root.as_deref(), field);
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| serde_json::to_string_pretty(&value).unwrap_or_default());
            let input = if matches!(field.kind, Kind::Json) || field.key == "search.exclude" {
                FieldInput::Multi(cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(2, 5)
                        .default_value(text)
                }))
            } else {
                FieldInput::Line(cx.new(|cx| InputState::new(window, cx).default_value(text)))
            };
            self.inputs.insert(field.key, input);
        }
        self.feedback.clear();
        self.copied = None;
    }
}
fn effective_value(prefs: &Preferences, root: Option<&Path>, field: Field) -> Value {
    prefs
        .option(if field.app_only { None } else { root }, field.key)
        .cloned()
        .unwrap_or_else(|| {
            if field.key == "terminal.font_size" {
                json!(prefs.display.code_font_size.round() as usize)
            } else {
                default_value(field.key)
            }
        })
}
fn notify(view: &WeakEntity<Pokarak>, cx: &mut App) {
    let _ = view.update(cx, |_, cx| cx.notify());
}
fn feedback(state: &DialogState, view: &WeakEntity<Pokarak>, text: String, cx: &mut App) {
    state.borrow_mut().feedback = text;
    notify(view, cx);
}
fn save_option(
    view: &WeakEntity<Pokarak>,
    state: &DialogState,
    field: Field,
    value: Option<Value>,
    cx: &mut App,
) {
    let root = if field.app_only {
        None
    } else {
        state.borrow().root.clone()
    };
    let message = view
        .update(cx, |app, cx| {
            let previous = match &root {
                Some(root) => app
                    .prefs
                    .project_options
                    .get(root)
                    .and_then(|map| map.get(field.key))
                    .cloned(),
                None => app.prefs.options.get(field.key).cloned(),
            };
            match value {
                Some(value) => app.prefs.set_option(root.as_deref(), field.key, value),
                None => app.prefs.remove_option(root.as_deref(), field.key),
            }
            if let Err(error) = app.prefs.try_save() {
                match previous {
                    Some(value) => app.prefs.set_option(root.as_deref(), field.key, value),
                    None => app.prefs.remove_option(root.as_deref(), field.key),
                }
                return format!("설정을 저장하지 못했습니다: {error}");
            }
            cx.notify();
            format!(
                "{} · {}에 저장됨",
                field.label,
                if root.is_some() {
                    "프로젝트"
                } else {
                    "앱 전체"
                }
            )
        })
        .unwrap_or_else(|_| "설정 창을 다시 여세요.".into());
    state.borrow_mut().feedback = message;
}
fn field_row(
    field: Field,
    prefs: &Preferences,
    state: &DialogState,
    view: &WeakEntity<Pokarak>,
    cx: &App,
) -> Div {
    let root = state.borrow().root.clone();
    let value = effective_value(prefs, root.as_deref(), field);
    let app_only = field.app_only && root.is_some();
    let inherited = root.as_ref().is_some_and(|root| {
        !prefs
            .project_options
            .get(root)
            .is_some_and(|map| map.contains_key(field.key))
    });
    let palette = Palette::new(cx);
    let scope = if field.app_only {
        "앱 전체"
    } else if root.is_some() && inherited {
        "앱 기본값 상속"
    } else if root.is_some() {
        "프로젝트 재정의"
    } else {
        "앱 기본값"
    };
    let mut controls = div().flex().items_center().gap_1();
    match field.kind {
        Kind::Bool => {
            let current = value.as_bool().unwrap_or(false);
            let (state, view) = (state.clone(), view.clone());
            let label = match field.key {
                "review.split_diff" => {
                    if current {
                        "좌우"
                    } else {
                        "통합"
                    }
                }
                _ => {
                    if current {
                        "켜짐"
                    } else {
                        "꺼짐"
                    }
                }
            };
            controls = controls.child(
                Button::new(field.key)
                    .small()
                    .label(label)
                    .selected(current)
                    .disabled(app_only)
                    .accessibility_label(format!("{} {label}", field.label))
                    .on_click(move |_, _, cx| {
                        save_option(&view, &state, field, Some(json!(!current)), cx)
                    }),
            );
        }
        Kind::Choice(choices) => {
            for &(key, label) in choices {
                let (state, view) = (state.clone(), view.clone());
                controls = controls.child(
                    Button::new(SharedString::from(format!("{}-{key}", field.key)))
                        .small()
                        .label(label)
                        .selected(value.as_str() == Some(key))
                        .disabled(app_only)
                        .on_click(move |_, _, cx| {
                            save_option(&view, &state, field, Some(json!(key)), cx)
                        }),
                );
            }
        }
        _ => {
            let input = state.borrow().inputs[&field.key].clone();
            let (state, view) = (state.clone(), view.clone());
            controls = controls.child(
                Button::new(SharedString::from(format!("apply-{}", field.key)))
                    .small()
                    .label("적용")
                    .disabled(app_only)
                    .on_click(
                        move |_, _, cx| match schema::parse(field, &input.value(cx)) {
                            Ok(value) => {
                                if field.key == "keyboard.bindings"
                                    && let Err(error) = validate_bindings(&value, cx)
                                {
                                    feedback(&state, &view, error, cx);
                                    return;
                                }
                                save_option(&view, &state, field, Some(value), cx);
                            }
                            Err(error) => feedback(&state, &view, error, cx),
                        },
                    ),
            );
        }
    }
    let (reset_state, reset_view) = (state.clone(), view.clone());
    controls = controls.child(
        Button::new(SharedString::from(format!("reset-{}", field.key)))
            .small()
            .ghost()
            .label(if root.is_some() && !field.app_only {
                "상속"
            } else {
                "기본값"
            })
            .disabled(app_only)
            .tooltip("이 항목의 재정의만 제거합니다")
            .on_click(move |_, window, cx| {
                save_option(&reset_view, &reset_state, field, None, cx);
                let _ = reset_view.update(cx, |app, cx| {
                    reset_state.borrow_mut().load_inputs(&app.prefs, window, cx)
                });
                notify(&reset_view, cx);
            }),
    );
    let mut row = div()
        .v_flex()
        .gap_1()
        .py_2()
        .border_b_1()
        .border_color(palette.rgb(LINE))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child(field.label),
                )
                .child(controls),
        )
        .child(
            div()
                .text_size(ui_text(11.))
                .text_color(palette.rgb(MUTED))
                .child(format!("{scope} · {}", field.hint)),
        );
    if matches!(field.kind, Kind::Text | Kind::Json | Kind::Number(..)) {
        row = row.child(
            state.borrow().inputs[&field.key]
                .element(SharedString::from(format!("entry-{}", field.key)), app_only),
        );
    }
    row
}

impl Pokarak {
    fn set_display(&mut self, display: DisplaySettings, cx: &mut Context<Self>) {
        self.prefs.display = display;
        self.prefs.save();
        theme::apply_display(display, cx);
        cx.notify();
    }
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_section("appearance", window, cx);
    }
    /// Reusable native entry point for command palette, tasks and extension management.
    pub(super) fn open_settings_section(
        &mut self,
        section: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation_request += 1;
        self.navigating = false;
        let state = Rc::new(RefCell::new(SettingsDialog {
            category: Category::ALL
                .into_iter()
                .find(|category| category.id() == section)
                .unwrap_or(Category::Appearance),
            root: None,
            project: self.current_project.clone(),
            worktree: self.current.clone(),
            inputs: HashMap::new(),
            keyboard_filter: cx
                .new(|cx| InputState::new(window, cx).placeholder("단축키·명령 검색")),
            feedback: String::new(),
            command_details: false,
            copied: None,
        }));
        state.borrow_mut().load_inputs(&self.prefs, window, cx);
        cx.subscribe(
            &state.borrow().keyboard_filter,
            |_, _, _: &InputEvent, cx| cx.notify(),
        )
        .detach();
        let weak = cx.weak_entity();
        window.open_app_dialog(cx,move |dialog,window,cx| {
            let Some(entity)=weak.upgrade() else{return dialog;};
            let app=entity.read(cx);
            let palette=Palette::new(cx);
            let category=state.borrow().category;
            let root=state.borrow().root.clone();
            let project=state.borrow().project.clone();
            let mut categories=div().v_flex().gap_1().w(ui_px(135.)).flex_none();
            for category in Category::ALL {
                let (state,view)=(state.clone(),weak.clone());
                let selected=category==state.borrow().category;
                categories=categories.child(Button::new(category.id()).small().ghost().accessibility_label(category.label()).child(div().w_full().text_left().child(category.label())).selected(selected).on_click(move |_,_,cx|{ state.borrow_mut().category=category; notify(&view,cx); }));
            }
            let mut scope=div().flex().items_center().gap_2();
            for (is_project,label) in [(false,"앱 전체"),(true,"이 프로젝트")] {
                let (state,view)=(state.clone(),weak.clone());
                let project=project.clone();
                scope=scope.child(Button::new(("settings-scope",usize::from(is_project))).small().label(label).selected(root.is_some()==is_project).disabled(is_project && project.is_none()).on_click(move |_,window,cx|{
                    state.borrow_mut().root=if is_project{project.clone()}else{None};
                    let _=view.update(cx,|app,cx|state.borrow_mut().load_inputs(&app.prefs,window,cx));
                    notify(&view,cx);
                }));
            }
            let scope_text=root.as_ref().map(|root|format!("프로젝트: {} · 별도 지정하지 않은 항목은 앱 설정을 상속합니다.",root.display())).unwrap_or_else(||"앱 기본값 · 이 설정은 검사 중인 저장소 밖에 저장됩니다.".into());
            let mut content=div().id(SharedString::from(format!("settings-scroll-{}-{}",category.id(),root.is_some()))).v_flex().flex_1().min_w_0().gap_1().max_h(window.viewport_size().height-ui_px(255.)).overflow_y_scroll().child(div().font_weight(FontWeight::SEMIBOLD).child(category.label()));
            if category==Category::Appearance {
                content=content.child(appearance(app,&weak,cx)).child(app_icons(app,&weak,&state,cx));
            }
            for &field in FIELDS.iter().filter(|field|field.category==category) {content=content.child(field_row(field,&app.prefs,&state,&weak,cx));}
            if category==Category::Language {content=content.child(app.language_manager_settings(weak.clone(),cx)).child(trust_settings(app,&weak,&state,cx));}
            if category==Category::Keyboard {content=content.child(keyboard_settings(app,&state,cx));}
            if category==Category::Ai {content=content.child(ai_settings(app,&weak,&state,cx));}
            if category==Category::Execution {content=content.children(management_button("forkarak_tasks::ManageTasks","실행 프로필·작업 관리 열기",cx));}
            if category==Category::Extensions {content=content.children(management_button("forkarak_extensions::ManageExtensions","확장 설치·업데이트·제거 관리",cx)).child(div().text_size(ui_text(12.)).child("설치·업데이트·제거는 확장 관리 화면에서 수행합니다. 여기에서는 프로젝트별 사용 여부와 확장 설정을 편집합니다."));}
            if category==Category::Updates {content=content.child(app.update_settings(weak.clone(),cx));}
            let (reset,reset_state)=(weak.clone(),state.clone());
            let reset_label=format!("{} · {} 초기화",if root.is_some(){"프로젝트 재정의"}else{"앱 기본값"},category.label());
            let feedback=state.borrow().feedback.clone();
            dialog.title("설정").width(ui_px(860.).min(window.viewport_size().width-ui_px(24.)))
                .child(div().v_flex().gap_2().child(scope).child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child(scope_text)).child(div().flex().gap_3().child(categories).child(content)))
                .footer(div().v_flex().gap_2().child(div().text_size(ui_text(12.)).text_color(palette.rgb(BLUE)).child(feedback)).child(div().flex().items_center().justify_between().child(Button::new("settings-category-reset").ghost().small().label(reset_label).on_click(move |_,window,cx|{
                    let root=reset_state.borrow().root.clone();
                    let message=reset.update(cx,|app,cx| {
                        let backup_options=app.prefs.options.clone(); let backup_projects=app.prefs.project_options.clone();
                        for field in FIELDS.iter().filter(|field|field.category==category && !(root.is_some() && field.app_only)) {app.prefs.remove_option(root.as_deref(),field.key);}
                        if category==Category::Language {app.prefs.remove_option(root.as_deref(),"language.enabled");app.cancel_managed_language_request();}
                        let old_display=app.prefs.display; let old_theme=app.prefs.theme;
                        if category==Category::Appearance && root.is_none() {app.prefs.display=DisplaySettings::DEFAULT;app.prefs.theme=ThemeChoice::System;app.prefs.remove_option(None,crate::app_icon::PREFERENCE_KEY);}
                        if let Err(error)=app.prefs.try_save(){app.prefs.options=backup_options;app.prefs.project_options=backup_projects;app.prefs.display=old_display;app.prefs.theme=old_theme;return format!("초기화를 저장하지 못했습니다: {error}");}
                        if category==Category::Appearance && root.is_none(){theme::apply(app.prefs.theme,app.prefs.display,window,cx);if let Err(error)=crate::app_icon::apply(app.prefs.app_icon()){return format!("설정 초기화됨 · 앱 아이콘 복원 실패: {error}");}}
                        cx.notify(); format!("{} · {} 초기화됨",if root.is_some(){"프로젝트 재정의"}else{"앱 설정"},category.label())
                    }).unwrap_or_default();
                    let _=reset.update(cx,|app,cx|reset_state.borrow_mut().load_inputs(&app.prefs,window,cx)); reset_state.borrow_mut().feedback=message;notify(&reset,cx);
                })).child(Button::new("settings-close").label("닫기").on_click(|_,window,cx|window.close_dialog(cx)))))
        });
    }
}
/// Runtime workers register these actions; a control is shown only for an available handler.
/// The manager can return here via open_settings_section("execution" / "extensions").
fn management_button(action: &str, label: &'static str, cx: &App) -> Option<Button> {
    let action = cx.build_action(action, None).ok()?;
    Some(
        Button::new(label)
            .small()
            .label(label)
            .on_click(move |_, window, cx| {
                window.close_dialog(cx);
                window.dispatch_action(action.boxed_clone(), cx);
            }),
    )
}

fn appearance(app: &Pokarak, view: &WeakEntity<Pokarak>, cx: &App) -> Div {
    let display = app.prefs.display;
    let palette = Palette::new(cx);
    let mut themes = div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().flex_1().child("테마 · 앱 전체"));
    for (index, (theme, label)) in [
        (ThemeChoice::System, "시스템"),
        (ThemeChoice::Light, "라이트"),
        (ThemeChoice::Dark, "다크"),
    ]
    .into_iter()
    .enumerate()
    {
        let view = view.clone();
        themes = themes.child(
            Button::new(("theme", index))
                .small()
                .label(label)
                .selected(app.prefs.theme == theme)
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |app, cx| app.set_theme(theme, window, cx));
                }),
        );
    }
    let mut preview = div()
        .v_flex()
        .gap_1()
        .p_2()
        .border_1()
        .border_color(palette.rgb(LINE))
        .child(
            div()
                .text_size(ui_text(display.ui_font_size))
                .child("일반 글꼴 · Forkarak 포카락 · 파일과 변경 사항"),
        );
    for (sign, text, bg, color) in [
        ("− ", "let count = 1;", patch::DELETE_BG, RED),
        ("+ ", "let count = 2;", patch::ADD_BG, GREEN),
    ] {
        let syntax = syntax::CodeSyntax::new(Path::new("Preview.rs"), text);
        preview = preview.child(
            div()
                .flex()
                .font_family("JetBrains Mono")
                .text_size(ui_px(display.code_font_size))
                .bg(palette.rgb(bg))
                .child(div().text_color(palette.rgb(color)).child(sign))
                .child(syntax.styled_line(0, text, false, cx)),
        );
    }
    div().v_flex().gap_2().child(themes).child(Setting::Zoom.row(display,view)).child(Setting::InterfaceFont.row(display,view)).child(Setting::CodeFont.row(display,view)).child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child("배율은 화면 전체의 크기·간격을 바꿉니다. 일반 글자는 UI에, 코드 글자는 코드·diff·Blame에 적용됩니다. 터미널은 터미널·실행에서 별도로 조절합니다. 표시 설정은 앱 전체에 적용됩니다.")).child(preview)
}

fn app_icons(app: &Pokarak, view: &WeakEntity<Pokarak>, state: &DialogState, cx: &App) -> Div {
    use crate::app_icon::{self, IconChoice};
    let palette = Palette::new(cx);
    let mut choices = div().flex().flex_wrap().gap_1();
    for choice in IconChoice::ALL {
        let selected = app.prefs.app_icon() == choice;
        let (view, state) = (view.clone(), state.clone());
        choices = choices.child(
            Button::new(SharedString::from(format!("app-icon-{}", choice.id())))
                .ghost()
                .selected(selected)
                .w(ui_px(80.))
                .h(ui_px(110.))
                .px_1()
                .accessibility_label(format!(
                    "앱 아이콘 · {}{}",
                    choice.label(),
                    if selected { " · 선택됨" } else { "" }
                ))
                .tooltip(if choice == IconChoice::FollowDefault {
                    "업데이트에 포함된 기본 아이콘을 따릅니다"
                } else {
                    "이 색상을 유지합니다 · Dock에 바로 적용"
                })
                .child(
                    div()
                        .v_flex()
                        .items_center()
                        .gap_1()
                        .child(img(choice.preview_path()).w(ui_px(56.)).h(ui_px(56.)))
                        .child(div().text_size(ui_text(10.)).child(choice.label()))
                        .child(div().text_size(ui_text(10.)).child(if selected {
                            "✓ 선택됨"
                        } else {
                            " "
                        })),
                )
                .on_click(move |_, _, cx| {
                    let message = view
                        .update(cx, |app, cx| {
                            let previous = app.prefs.app_icon();
                            let saved = app.prefs.options.get(app_icon::PREFERENCE_KEY).cloned();
                            if let Err(error) = app_icon::apply(choice) {
                                return format!("앱 아이콘을 변경하지 못했습니다: {error}");
                            }
                            app.prefs.set_app_icon(choice);
                            if let Err(error) = app.prefs.try_save() {
                                match saved {
                                    Some(value) => {
                                        app.prefs.set_option(None, app_icon::PREFERENCE_KEY, value)
                                    }
                                    None => app.prefs.remove_option(None, app_icon::PREFERENCE_KEY),
                                }
                                let _ = app_icon::apply(previous);
                                return format!("앱 아이콘을 저장하지 못했습니다: {error}");
                            }
                            cx.notify();
                            format!("앱 아이콘 · {} 적용됨", choice.label())
                        })
                        .unwrap_or_default();
                    feedback(&state, &view, message, cx);
                }),
        );
    }
    div().v_flex().gap_1().py_2().border_b_1().border_color(palette.rgb(LINE))
        .child(div().font_weight(FontWeight::MEDIUM).child("앱 아이콘 · 앱 전체"))
        .child(choices)
        .child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED))
            .child("Dock에 바로 적용되며 다음 실행에도 유지됩니다. ‘기본값 따르기’는 업데이트의 기본 아이콘을 따릅니다. Finder 아이콘은 앱에 포함된 기본 아이콘을 사용합니다."))
}
fn trust_settings(app: &Pokarak, view: &WeakEntity<Pokarak>, state: &DialogState, cx: &App) -> Div {
    let mut section=div().v_flex().gap_2().pt_2().child("분석 허용 워크트리").child(div().text_size(ui_text(11.)).child("허용 추가는 해당 워크트리에서 언어 분석을 켤 때 확인합니다. 해제하면 실행 중인 해당 분석을 중지합니다."));
    if app.prefs.trusted.is_empty() {
        section = section.child("허용된 워크트리가 없습니다.");
    }
    for (index, root) in app.prefs.trusted.iter().enumerate() {
        let (root, view, state) = (root.clone(), view.clone(), state.clone());
        section = section.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(root.display().to_string()),
                )
                .child(
                    Button::new(("revoke-trust", index))
                        .small()
                        .label("허용 해제")
                        .on_click(move |_, _, cx| {
                            let message = view
                                .update(cx, |app, cx| {
                                    let previous = app.prefs.trusted.clone();
                                    app.prefs.trusted.retain(|path| path != &root);
                                    if let Err(error) = app.prefs.try_save() {
                                        app.prefs.trusted = previous;
                                        return format!("허용 해제를 저장하지 못했습니다: {error}");
                                    }
                                    if app.current.as_ref() == Some(&root) {
                                        app.stop_analysis(cx);
                                    }
                                    cx.notify();
                                    format!("분석 허용 해제됨 · {}", root.display())
                                })
                                .unwrap_or_default();
                            feedback(&state, &view, message, cx);
                        }),
                ),
        );
    }
    let _ = cx;
    section
}
// Defaults include editor/search/worktree commands so conflicts are visible together.
const KEYBOARD_DEFAULTS: &[(&str, &str, &str)] = &[
    ("find_file", "파일 찾기", "cmd-p"),
    ("find_text", "프로젝트 검색", "cmd-shift-f"),
    ("find_definition", "정의로 이동", "cmd-b"),
    ("find_references", "사용처 찾기", "alt-f7"),
    ("next_worktree", "다음 워크트리", "cmd-alt-down"),
    ("previous_worktree", "이전 워크트리", "cmd-alt-up"),
    ("save_file", "파일 저장", "cmd-s"),
    ("toggle_code", "코드/diff", "cmd-alt-d"),
    ("replace_file", "파일 내 바꾸기", "cmd-alt-f"),
    ("select_next_word", "다음 같은 단어 선택", "cmd-d"),
    ("add_cursor_above", "위에 커서 추가", "ctrl-alt-up"),
    ("add_cursor_below", "아래에 커서 추가", "ctrl-alt-down"),
    ("move_line_up", "줄 위로 이동", "alt-up"),
    ("move_line_down", "줄 아래로 이동", "alt-down"),
    ("duplicate_line_up", "줄 위로 복제", "alt-shift-up"),
    ("duplicate_line_down", "줄 아래로 복제", "alt-shift-down"),
    ("go_to_line", "줄로 이동", "cmd-l"),
    ("word_wrap", "화면 줄바꿈", "alt-z"),
    ("command_palette", "명령 팔레트", "cmd-shift-p"),
    ("toggle_comment", "주석 토글", "cmd-/"),
    ("pin_tab", "탭 고정", "cmd-shift-enter"),
    ("format_file", "파일 포맷", "alt-shift-f"),
];
fn binding_conflicts(value: &Value) -> Vec<String> {
    let mut keys: HashMap<String, Vec<String>> = HashMap::new();
    for &(id, label, key) in KEYBOARD_DEFAULTS {
        let key = value.get(id).and_then(Value::as_str).unwrap_or(key);
        if !key.is_empty() {
            keys.entry(normalize_key(key))
                .or_default()
                .push(label.into());
        }
    }
    if let Some(map) = value.as_object() {
        for (id, key) in map {
            if KEYBOARD_DEFAULTS.iter().any(|(known, _, _)| known == id) {
                continue;
            }
            if let Some(key) = key.as_str().filter(|key| !key.is_empty()) {
                keys.entry(normalize_key(key)).or_default().push(id.clone());
            }
        }
    }
    let mut conflicts: Vec<_> = keys
        .into_iter()
        .filter(|(_, commands)| commands.len() > 1)
        .map(|(key, commands)| format!("{key}: {}", commands.join(" · ")))
        .collect();
    conflicts.sort();
    conflicts
}
fn normalize_key(key: &str) -> String {
    key.split_whitespace()
        .map(|stroke| {
            let mut parts: Vec<_> = stroke
                .split('-')
                .map(|part| match part.to_lowercase().as_str() {
                    "command" | "super" => "cmd".to_owned(),
                    "option" => "alt".to_owned(),
                    "control" => "ctrl".to_owned(),
                    other => other.to_owned(),
                })
                .collect();
            if parts.len() > 1 {
                let last = parts.pop().unwrap();
                parts.sort();
                parts.push(last);
            }
            parts.join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn validate_bindings(value: &Value, cx: &App) -> Result<(), String> {
    let Some(map) = value.as_object() else {
        return Err("명령 ID와 단축키의 JSON 객체를 입력하세요.".into());
    };
    for (id, key) in map {
        let key = key.as_str().ok_or("단축키는 문자열이어야 합니다.")?;
        if key.trim().is_empty() {
            return Err("빈 단축키 대신 항목을 삭제하여 기본값을 복원하세요.".into());
        }
        if !KEYBOARD_DEFAULTS.iter().any(|(known, _, _)| known == id) {
            return Err(format!("등록되지 않은 명령입니다: {id}"));
        }
        let action = Box::new(FindFile);
        KeyBinding::load(key, action, None, false, None, cx)
            .map_err(|error| format!("{id}: {error}"))?;
    }
    let conflicts = binding_conflicts(value);
    if !conflicts.is_empty() {
        return Err(format!("단축키 충돌 · {}", conflicts.join(" / ")));
    }
    Ok(())
}
fn keyboard_settings(app: &Pokarak, state: &DialogState, cx: &App) -> Div {
    let root = state.borrow().root.clone();
    let value = app
        .prefs
        .option(root.as_deref(), "keyboard.bindings")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let query = state
        .borrow()
        .keyboard_filter
        .read(cx)
        .value()
        .to_lowercase();
    let mut section = div()
        .v_flex()
        .gap_1()
        .pt_2()
        .child(Input::new(&state.borrow().keyboard_filter))
        .child("등록된 기본 단축키 · 저장한 재정의는 명령 등록 시 적용됩니다.");
    for &(id, label, key) in KEYBOARD_DEFAULTS {
        let key = value.get(id).and_then(Value::as_str).unwrap_or(key);
        if !format!("{id} {label} {key}")
            .to_lowercase()
            .contains(&query)
        {
            continue;
        }
        section = section.child(
            div()
                .text_size(ui_text(11.))
                .child(format!("{label} · {key} · {id}")),
        );
    }
    let conflicts = binding_conflicts(&value);
    if conflicts.is_empty() {
        section = section.child("✓ 편집·검색·다중 선택·워크트리 단축키 충돌 없음");
    } else {
        for conflict in conflicts {
            section = section.child(div().text_color(Palette::new(cx).rgb(RED)).child(conflict));
        }
    }
    section
}
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
fn agent_prompt(repo: &Path, agent: &str) -> String {
    format!(
        "Use Forkarak CLI for this worktree: {}. First run forkarak ctl instructions --agent {agent}, then forkarak ctl status --repo {}. Read the JSON instructions and follow them. All ctl responses are JSON; verify ok and the canonical repo. Use read-only inspection first. File edits, staging, commits and merges require my explicit request and --yes. Save only with exact expected content. Do not force, discard, reset, stash, checkout, start builds or language servers implicitly.",
        repo.display(),
        shell_quote(&repo.to_string_lossy())
    )
}
fn ai_command(prefs: &Preferences, project: Option<&Path>, worktree: &Path) -> (String, String) {
    let cli = prefs.option_string(project, "ai.cli", "codex");
    let path = prefs.option_string(project, "ai.path", "");
    let executable = if path.trim().is_empty() {
        cli.clone()
    } else {
        path
    };
    let mut args: Vec<String> = prefs
        .option(project, "ai.args")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    args.push(agent_prompt(worktree, &cli));
    let invocation = std::iter::once(executable)
        .chain(args)
        .map(|arg| shell_quote(&arg))
        .collect::<Vec<_>>()
        .join(" ");
    (
        format!(
            "cd -- {} && {invocation}",
            shell_quote(&worktree.to_string_lossy())
        ),
        invocation,
    )
}
fn copy_button(
    id: &'static str,
    label: &'static str,
    text: String,
    state: &DialogState,
    view: &WeakEntity<Pokarak>,
) -> Button {
    let (state, view) = (state.clone(), view.clone());
    let copied = state.borrow().copied == Some(id);
    Button::new(id)
        .small()
        .label(if copied { "✓ 복사됨" } else { label })
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            state.borrow_mut().copied = Some(id);
            feedback(&state, &view, "✓ 클립보드에 복사했습니다.".into(), cx);
        })
}
fn ai_settings(app: &Pokarak, view: &WeakEntity<Pokarak>, state: &DialogState, cx: &App) -> Div {
    let palette = Palette::new(cx);
    let project = state.borrow().project.clone();
    let worktree = state.borrow().worktree.clone();
    let mut section = div().v_flex().gap_2().pt_2();
    let Some(worktree) = worktree else {
        return section.child("프로젝트를 열면 AI 실행 대상 워크트리가 표시됩니다.");
    };
    let (command, invocation) = ai_command(&app.prefs, project.as_deref(), &worktree);
    let start = app
        .prefs
        .option_string(project.as_deref(), "ai.start", "terminal");
    let current = app.current.as_ref() == Some(&worktree)
        && app.current_project == project
        && app.opening_path.is_none();
    section=section.child(div().font_weight(FontWeight::MEDIUM).child(format!("실행 대상 워크트리: {}",worktree.display()))).child(div().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child("시작은 현재 선택한 워크트리의 새 세션을 엽니다. 경로와 인수는 개별 인용하며 기존 터미널에 입력하지 않습니다."));
    let (launch_view, launch_state, launch_root, launch_project) = (
        view.clone(),
        state.clone(),
        worktree.clone(),
        project.clone(),
    );
    let copy = command.clone();
    let launch = Button::new("ai-start")
        .small()
        .label(if start == "copy" {
            "시작 명령 복사"
        } else {
            "AI 시작"
        })
        .disabled(!current)
        .on_click(move |_, window, cx| {
            if start == "copy" {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                launch_state.borrow_mut().copied = Some("ai-start");
                feedback(
                    &launch_state,
                    &launch_view,
                    "✓ 시작 명령을 복사했습니다.".into(),
                    cx,
                );
                return;
            }
            let message = launch_view
                .update(cx, |app, cx| {
                    if app.current.as_ref() != Some(&launch_root)
                        || app.current_project != launch_project
                        || app.opening_path.is_some()
                    {
                        return "워크트리가 바뀌었습니다. 설정을 다시 여세요.".to_owned();
                    }
                    #[cfg(target_os = "macos")]
                    {
                        window.close_dialog(cx);
                        app.start_worktree_command(&launch_root, invocation.clone(), window, cx);
                        format!("AI 세션 시작 · {}", launch_root.display())
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = window;
                        "이 플랫폼에서는 시작 명령 복사를 사용하세요.".to_owned()
                    }
                })
                .unwrap_or_default();
            feedback(&launch_state, &launch_view, message, cx);
        });
    section = section.child(
        div()
            .flex()
            .gap_2()
            .child(launch)
            .child(copy_button(
                "ai-copy-command",
                "명령 복사",
                command.clone(),
                state,
                view,
            ))
            .child(copy_button(
                "ai-copy-prompt",
                "프롬프트 복사",
                agent_prompt(
                    &worktree,
                    &app.prefs
                        .option_string(project.as_deref(), "ai.cli", "codex"),
                ),
                state,
                view,
            )),
    );
    let (details_state, details_view) = (state.clone(), view.clone());
    let details = state.borrow().command_details;
    section = section.child(
        Button::new("ai-command-details")
            .small()
            .ghost()
            .label(if details {
                "명령 자세히 접기"
            } else {
                "명령 자세히 보기"
            })
            .on_click(move |_, _, cx| {
                details_state.borrow_mut().command_details = !details;
                notify(&details_view, cx);
            }),
    );
    if details {
        section = section
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(ui_text(11.))
                    .child(command),
            )
            .child(copy_button(
                "ai-copy-status",
                "상태 확인 명령 복사",
                format!(
                    "forkarak ctl status --repo {}",
                    shell_quote(&worktree.to_string_lossy())
                ),
                state,
                view,
            ));
    }
    section.child(div().text_size(ui_text(11.)).child("실행 옵션은 이 대상을 소유한 프로젝트 설정을 사용합니다. 앱 기본값 편집도 프로젝트 재정의보다 우선하지 않습니다."))
}
#[cfg(test)]
mod tests {
    use super::{ai_command, binding_conflicts, shell_quote};
    use crate::prefs::Preferences;
    use serde_json::json;
    use std::path::Path;
    #[test]
    fn shell_arguments_cannot_escape_worktree_or_prompt() {
        assert_eq!(shell_quote("a'b $(touch x)"), "'a'\\''b $(touch x)'");
        let mut prefs = Preferences::default();
        prefs.set_option(None, "ai.args", json!(["--name", "x'; touch bad"]));
        let (command, invocation) =
            ai_command(&prefs, None, Path::new("/tmp/project with 'quotes"));
        assert!(command.starts_with(
            "cd -- '/tmp/project with '\\''quotes' && 'codex' '--name' 'x'\\''; touch bad'"
        ));
        assert!(!invocation.starts_with("cd"));
    }
    #[test]
    fn login_shell_launch_uses_exact_target_and_literal_arguments() {
        use std::{fs, os::unix::fs::PermissionsExt, process::Command};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project ' literal");
        fs::create_dir(&root).unwrap();
        let executable = temp.path().join("fake cli");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > \"$AI_CAPTURE\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut prefs = Preferences::default();
        prefs.set_option(None, "ai.path", json!(executable));
        prefs.set_option(
            None,
            "ai.args",
            json!(["literal ; $(touch unwanted)", "quote'arg"]),
        );
        let (command, _) = ai_command(&prefs, None, &root);
        let capture = temp.path().join("capture");
        let output = Command::new("/bin/zsh")
            .args(["-l", "-i", "-c", &command])
            .env("HOME", temp.path())
            .env("ZDOTDIR", temp.path())
            .env("AI_CAPTURE", &capture)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let captured = fs::read_to_string(capture).unwrap();
        let lines: Vec<_> = captured.lines().collect();
        assert_eq!(
            Path::new(lines[0]).canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        assert_eq!(lines[1], "literal ; $(touch unwanted)");
        assert_eq!(lines[2], "quote'arg");
        assert!(lines[3].contains("ctl instructions --agent codex"));
        assert!(!root.join("unwanted").exists());
    }

    #[test]
    fn conflicts_compare_modifier_order_and_defaults() {
        let value = json!({"find_text":"alt-cmd-down"});
        assert!(
            binding_conflicts(&value)
                .iter()
                .any(|conflict| conflict.contains("다음 워크트리"))
        );
        assert!(binding_conflicts(&json!({})).is_empty());
    }
}
