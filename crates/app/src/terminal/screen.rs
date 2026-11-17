use super::Listener;
use alacritty_terminal::{
    Term,
    event::WindowSize,
    grid::Dimensions,
    term::{cell::Flags, color::Colors},
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use gpui_kit::*;

const PALETTE: [u32; 16] = [
    0x20252c, 0xdf8789, 0x8dc7a4, 0xdfc286, 0x83b3df, 0xbf9fda, 0x7cc7cd, 0xd1d7df, 0x697581,
    0xffa2a4, 0xacdfb9, 0xffdda0, 0xa1cfff, 0xdebbfa, 0x9de3e7, 0xf3f6fa,
];
pub(super) const BACKGROUND: u32 = 0x191d23;
pub(super) const FOREGROUND: u32 = 0xdde4ec;
pub(super) fn default_color(index: usize) -> Rgb {
    let value = match index {
        0..16 => PALETTE[index],
        16..232 => {
            let n = index - 16;
            let channel = |v: usize| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (channel(n / 36) << 16) | (channel((n / 6) % 6) << 8) | channel(n % 6)
        }
        232..256 => ((8 + (index - 232) * 10) as u32) * 0x010101,
        256 => FOREGROUND,
        257 => BACKGROUND,
        259..267 => PALETTE[index - 259],
        _ => FOREGROUND,
    };
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
fn resolve(color: Color, colors: &Colors) -> u32 {
    let rgb = match color {
        Color::Spec(rgb) => rgb,
        Color::Named(named) => colors[named].unwrap_or_else(|| default_color(named as usize)),
        Color::Indexed(i) => colors[i as usize].unwrap_or_else(|| default_color(i as usize)),
    };
    (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
}

#[derive(Clone, Copy, Default)]
pub(super) struct Metrics {
    pub bounds: Bounds<Pixels>,
    pub cell: Size<Pixels>,
    pub font_size: Pixels,
}
impl Metrics {
    pub fn measured(bounds: Bounds<Pixels>, font_size: Pixels, w: &Window) -> Self {
        let text = w.text_system();
        let width = text
            .advance(text.resolve_font(&font("JetBrains Mono")), font_size, 'M')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        Self {
            bounds,
            cell: size(width.max(px(1.)), (font_size * 1.42).ceil()),
            font_size,
        }
    }
    pub fn size(self) -> WindowSize {
        WindowSize {
            num_cols: ((self.bounds.size.width / self.cell.width).floor() as u16).clamp(2, 1000),
            num_lines: ((self.bounds.size.height / self.cell.height).floor() as u16).clamp(1, 500),
            cell_width: f32::from(self.cell.width).round() as u16,
            cell_height: f32::from(self.cell.height).round() as u16,
        }
    }
    pub fn coords(self, point: Point<Pixels>) -> (usize, usize) {
        (
            ((point.x - self.bounds.left()) / self.cell.width)
                .floor()
                .max(0.) as usize,
            ((point.y - self.bounds.top()) / self.cell.height)
                .floor()
                .max(0.) as usize,
        )
    }
    pub fn at(self, col: usize, row: usize) -> Point<Pixels> {
        self.bounds.origin + point(self.cell.width * col as f32, self.cell.height * row as f32)
    }
}

#[derive(PartialEq)]
struct Style {
    fg: u32,
    bg: u32,
    flags: Flags,
}
struct Run {
    col: usize,
    row: usize,
    cells: usize,
    style: Style,
    text: String,
}
pub(super) struct Frame {
    runs: Vec<Run>,
    pub cursor: Option<(usize, usize, CursorShape)>,
    background: u32,
}
impl Frame {
    pub fn read(term: &Term<Listener>, focused: bool) -> Self {
        let screen = term.renderable_content();
        let background = resolve(Color::Named(NamedColor::Background), screen.colors);
        let cursor_row = screen.cursor.point.line.0 + screen.display_offset as i32;
        let cursor = (cursor_row >= 0
            && (cursor_row as usize) < term.screen_lines()
            && screen.cursor.shape != CursorShape::Hidden)
            .then_some((
                screen.cursor.point.column.0,
                cursor_row as usize,
                screen.cursor.shape,
            ));
        let mut runs: Vec<Run> = Vec::new();
        for indexed in screen.display_iter {
            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let row = (indexed.point.line.0 + screen.display_offset as i32) as usize;
            let col = indexed.point.column.0;
            let mut fg = resolve(cell.fg, screen.colors);
            let mut bg = resolve(cell.bg, screen.colors);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if screen
                .selection
                .is_some_and(|sel| sel.contains(indexed.point))
            {
                bg = 0x385976;
            }
            if focused && cursor == Some((col, row, CursorShape::Block)) {
                fg = background;
                bg = FOREGROUND;
            }
            let blank = cell.flags.contains(Flags::HIDDEN) || cell.c == ' ';
            let flags = cell.flags
                & (Flags::BOLD
                    | Flags::ITALIC
                    | Flags::ALL_UNDERLINES
                    | Flags::STRIKEOUT
                    | Flags::DIM);
            if blank
                && bg == background
                && !flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
            {
                continue;
            }
            let style = Style { fg, bg, flags };
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let previous = runs.last_mut().filter(|r| {
                r.row == row
                    && r.col + r.cells == col
                    && r.style == style
                    && r.text.len() == r.cells
                    && cell.c.is_ascii()
            });
            let run = if let Some(run) = previous {
                run
            } else {
                runs.push(Run {
                    col,
                    row,
                    cells: 0,
                    style,
                    text: String::new(),
                });
                runs.last_mut().unwrap()
            };
            run.cells += width;
            run.text.push(if blank { ' ' } else { cell.c });
            if !blank && let Some(extra) = cell.zerowidth() {
                run.text.extend(extra);
            }
        }
        Self {
            runs,
            cursor,
            background,
        }
    }
    pub fn paint(
        self,
        metrics: Metrics,
        marked: &str,
        focused: bool,
        w: &mut Window,
        cx: &mut App,
    ) {
        w.paint_quad(fill(metrics.bounds, rgb(self.background)));
        for run in self.runs {
            let origin = metrics.at(run.col, run.row);
            let width = metrics.cell.width * run.cells as f32;
            if run.style.bg != self.background {
                w.paint_quad(fill(
                    Bounds::new(origin, size(width, metrics.cell.height)),
                    rgb(run.style.bg),
                ));
            }
            let mut face = font("JetBrains Mono");
            if run.style.flags.contains(Flags::BOLD) {
                face.weight = FontWeight::BOLD;
            }
            if run.style.flags.contains(Flags::ITALIC) {
                face.style = FontStyle::Italic;
            }
            let ink = rgba(
                (run.style.fg << 8)
                    | if run.style.flags.contains(Flags::DIM) {
                        175
                    } else {
                        255
                    },
            );
            let style = TextRun {
                len: run.text.len(),
                font: face,
                color: ink.into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line =
                w.text_system()
                    .shape_line(run.text.into(), metrics.font_size, &[style], None);
            let _ = line.paint(origin, metrics.cell.height, TextAlign::Left, None, w, cx);
            if run
                .style
                .flags
                .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
            {
                let offset = if run.style.flags.contains(Flags::STRIKEOUT) {
                    metrics.cell.height * 0.52
                } else {
                    metrics.cell.height - px(2.)
                };
                w.paint_quad(fill(
                    Bounds::new(origin + point(px(0.), offset), size(width, px(1.))),
                    ink,
                ));
            }
        }
        if let Some((col, row, shape)) = self.cursor {
            let origin = metrics.at(col, row);
            let bounds = Bounds::new(origin, metrics.cell);
            match (focused, shape) {
                (true, CursorShape::Beam) => w.paint_quad(fill(
                    Bounds::new(origin, size(px(2.), metrics.cell.height)),
                    rgb(FOREGROUND),
                )),
                (true, CursorShape::Underline) => w.paint_quad(fill(
                    Bounds::new(
                        point(origin.x, bounds.bottom() - px(2.)),
                        size(metrics.cell.width, px(2.)),
                    ),
                    rgb(FOREGROUND),
                )),
                (false, _) | (_, CursorShape::HollowBlock) => w.paint_quad(quad(
                    bounds,
                    px(0.),
                    transparent_black(),
                    px(1.),
                    rgb(FOREGROUND),
                    BorderStyle::Solid,
                )),
                _ => {}
            }
            if !marked.is_empty() {
                let style = TextRun {
                    len: marked.len(),
                    font: font("JetBrains Mono"),
                    color: rgb(FOREGROUND).into(),
                    background_color: Some(rgb(0x30445a).into()),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        ..Default::default()
                    }),
                    strikethrough: None,
                };
                let line = w.text_system().shape_line(
                    marked.to_owned().into(),
                    metrics.font_size,
                    &[style],
                    None,
                );
                let _ = line.paint(origin, metrics.cell.height, TextAlign::Left, None, w, cx);
            }
        }
    }
}
