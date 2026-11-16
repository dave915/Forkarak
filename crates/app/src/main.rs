use gpui_kit::*;
use gpui_kit::component::{*, label::Label};

struct Pokarak;
impl Render for Pokarak {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x20252c)).text_color(rgb(0xe5eaf0)).v_flex().p_8()
            .child(Label::new("포카락"))
            .child(Label::new("워크트리의 변경사항을 읽습니다."))
    }
}
fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(900.)), cx))),
            ..Default::default()
        }, cx, |_, cx| cx.new(|_| Pokarak)).expect("포카락 창을 열 수 없습니다");
        cx.activate(true);
    });
}

