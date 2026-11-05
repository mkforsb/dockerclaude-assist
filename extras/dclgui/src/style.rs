//! The look of the GUI, approximating libadwaita in Iced.

use iced::theme::{Mode, Palette};
use iced::widget::scrollable::{self, AutoScroll, Rail, Scroller};
use iced::widget::{button, container, rule, svg, text, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector, border};

const BUTTON_RADIUS: f32 = 9.0;
const CARD_RADIUS: f32 = 12.0;
const DIALOG_RADIUS: f32 = 18.0;
const PILL_RADIUS: f32 = 999.0;

/// The named colors of libadwaita's stylesheet.
#[derive(Clone, Copy)]
struct Colors {
    window: Color,
    header: Color,
    card: Color,
    dialog: Color,
    fg: Color,
    accent: Color,
    destructive: Color,
    success: Color,
    warning: Color,
    shade: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

const LIGHT: Colors = Colors {
    window: rgb(0xfa, 0xfa, 0xfb),
    header: rgb(0xff, 0xff, 0xff),
    card: rgb(0xff, 0xff, 0xff),
    dialog: rgb(0xfa, 0xfa, 0xfb),
    fg: rgb(0x2e, 0x2e, 0x32),
    accent: rgb(0x35, 0x84, 0xe4),
    destructive: rgb(0xc3, 0x00, 0x00),
    success: rgb(0x26, 0xa2, 0x69),
    warning: rgb(0xe5, 0xa5, 0x0a),
    shade: Color::from_rgba(0.0, 0.0, 0.024, 0.12),
};

const DARK: Colors = Colors {
    window: rgb(0x22, 0x22, 0x26),
    header: rgb(0x2e, 0x2e, 0x32),
    card: rgb(0x33, 0x33, 0x37),
    dialog: rgb(0x36, 0x36, 0x3a),
    fg: rgb(0xff, 0xff, 0xff),
    accent: rgb(0x35, 0x84, 0xe4),
    destructive: rgb(0xff, 0x93, 0x8c),
    success: rgb(0x57, 0xe3, 0x89),
    warning: rgb(0xf8, 0xe4, 0x5c),
    shade: Color::from_rgba(0.0, 0.0, 0.024, 0.36),
};

/// Toasts are dark in light mode as well.
const TOAST: Color = rgb(0x24, 0x24, 0x28);

fn colors(theme: &Theme) -> Colors {
    if theme.extended_palette().is_dark {
        DARK
    } else {
        LIGHT
    }
}

/// The theme for a system preference of `mode`.
pub fn theme(mode: Mode) -> Theme {
    let (name, colors) = match mode {
        Mode::Dark => ("Adwaita Dark", DARK),
        Mode::Light | Mode::None => ("Adwaita", LIGHT),
    };

    Theme::custom(
        name,
        Palette {
            background: colors.window,
            text: colors.fg,
            primary: colors.accent,
            success: colors.success,
            warning: colors.warning,
            danger: colors.destructive,
        },
    )
}

// Buttons

/// A translucent fill of the foreground color, as in libadwaita. Disabled buttons
/// are simply half as opaque.
fn fill_button(
    fg: Color,
    rest: f32,
    hover: f32,
    pressed: f32,
    status: button::Status,
) -> button::Style {
    let (fill, text) = match status {
        button::Status::Active => (rest, fg),
        button::Status::Hovered => (hover, fg),
        button::Status::Pressed => (pressed, fg),
        button::Status::Disabled => (rest / 2.0, fg.scale_alpha(0.5)),
    };

    button::Style {
        background: (fill > 0.0).then(|| fg.scale_alpha(fill).into()),
        text_color: text,
        border: border::rounded(BUTTON_RADIUS),
        ..button::Style::default()
    }
}

pub fn regular(theme: &Theme, status: button::Status) -> button::Style {
    fill_button(colors(theme).fg, 0.1, 0.15, 0.3, status)
}

pub fn flat(theme: &Theme, status: button::Status) -> button::Style {
    fill_button(colors(theme).fg, 0.0, 0.07, 0.16, status)
}

fn circular(style: button::Style) -> button::Style {
    button::Style {
        border: border::rounded(PILL_RADIUS),
        ..style
    }
}

pub fn regular_circular(theme: &Theme, status: button::Status) -> button::Style {
    circular(regular(theme, status))
}

pub fn flat_circular(theme: &Theme, status: button::Status) -> button::Style {
    circular(flat(theme, status))
}

/// A button of a view switcher.
pub fn tab(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let rest = if selected { 0.1 } else { 0.0 };
        fill_button(colors(theme).fg, rest, rest + 0.05, rest + 0.15, status)
    }
}

/// The default action of a dialog.
pub fn suggested(theme: &Theme, status: button::Status) -> button::Style {
    let accent = colors(theme).accent;
    let (background, text) = match status {
        button::Status::Active => (accent, Color::WHITE),
        button::Status::Hovered => (accent.scale_alpha(0.9), Color::WHITE),
        button::Status::Pressed => (accent.scale_alpha(0.8), Color::WHITE),
        button::Status::Disabled => (accent.scale_alpha(0.5), Color::WHITE.scale_alpha(0.5)),
    };

    button::Style {
        background: Some(background.into()),
        text_color: text,
        border: border::rounded(BUTTON_RADIUS),
        ..button::Style::default()
    }
}

/// The destructive response of an alert dialog: neutral, with just the label red.
pub fn destructive(theme: &Theme, status: button::Status) -> button::Style {
    let colors = colors(theme);

    button::Style {
        text_color: colors.destructive,
        ..fill_button(colors.fg, 0.1, 0.15, 0.3, status)
    }
}

/// A flat button on a toast.
pub fn toast_button(_theme: &Theme, status: button::Status) -> button::Style {
    fill_button(Color::WHITE, 0.0, 0.1, 0.2, status)
}

// Containers

pub fn header(theme: &Theme) -> container::Style {
    container::Style::default().background(colors(theme).header)
}

pub fn card(theme: &Theme) -> container::Style {
    let colors = colors(theme);

    container::Style {
        background: Some(colors.card.into()),
        border: border::rounded(CARD_RADIUS),
        shadow: Shadow {
            color: colors.shade,
            offset: Vector::new(0.0, 1.0),
            blur_radius: 3.0,
        },
        ..container::Style::default()
    }
}

pub fn dialog(theme: &Theme) -> container::Style {
    let colors = colors(theme);

    container::Style {
        background: Some(colors.dialog.into()),
        border: border::rounded(DIALOG_RADIUS),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 24.0,
        },
        ..container::Style::default()
    }
}

/// The dimming behind a dialog.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style::default().background(Color::BLACK.scale_alpha(0.3))
}

pub fn toast(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(TOAST.into()),
        text_color: Some(Color::WHITE),
        border: border::rounded(DIALOG_RADIUS),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

pub fn tooltip(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(TOAST.into()),
        text_color: Some(Color::WHITE),
        border: border::rounded(BUTTON_RADIUS),
        ..container::Style::default()
    }
}

/// The count of problems next to the Doctor tab.
pub fn badge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(LIGHT.warning.into()),
        text_color: Some(rgb(0x24, 0x1f, 0x31)),
        border: border::rounded(PILL_RADIUS),
        ..container::Style::default()
    }
}

/// The dot showing whether a container is running.
pub fn status_dot(running: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let colors = colors(theme);

        container::Style {
            background: Some(if running {
                colors.success.into()
            } else {
                colors.fg.scale_alpha(0.3).into()
            }),
            border: border::rounded(PILL_RADIUS),
            ..container::Style::default()
        }
    }
}

// Text

pub fn dim(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(colors(theme).fg.scale_alpha(0.6)),
    }
}

pub fn error(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(colors(theme).destructive),
    }
}

pub fn toast_error(_theme: &Theme) -> text::Style {
    text::Style {
        color: Some(DARK.destructive),
    }
}

// Other widgets

pub fn separator(theme: &Theme) -> rule::Style {
    rule::Style {
        color: colors(theme).fg.scale_alpha(0.08),
        radius: border::Radius::default(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn entry(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let colors = colors(theme);
    let focused = matches!(status, text_input::Status::Focused { .. });

    text_input::Style {
        background: colors.fg.scale_alpha(0.08).into(),
        border: Border {
            color: if focused {
                colors.accent.scale_alpha(0.5)
            } else {
                Color::TRANSPARENT
            },
            width: 2.0,
            radius: BUTTON_RADIUS.into(),
        },
        icon: colors.fg,
        placeholder: colors.fg.scale_alpha(0.5),
        value: colors.fg,
        selection: colors.accent.scale_alpha(0.3),
    }
}

/// A thin floating scrollbar without a visible track.
pub fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let fg = colors(theme).fg;
    let opacity = match status {
        scrollable::Status::Active { .. } => 0.2,
        scrollable::Status::Hovered { .. } => 0.4,
        scrollable::Status::Dragged { .. } => 0.6,
    };

    let rail = Rail {
        background: None,
        border: Border::default(),
        scroller: Scroller {
            background: Background::Color(fg.scale_alpha(opacity)),
            border: border::rounded(PILL_RADIUS),
        },
    };

    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: AutoScroll {
            background: Background::Color(TOAST),
            border: border::rounded(PILL_RADIUS),
            shadow: Shadow::default(),
            icon: Color::WHITE,
        },
    }
}

// Icons

/// Symbolic icons, tinted to fit their surroundings.
#[derive(Clone, Copy)]
pub enum Icon {
    Add,
    Check,
    Close,
    Folder,
    Refresh,
    Start,
    Stop,
    Terminal,
}

impl Icon {
    fn body(self) -> &'static str {
        match self {
            Icon::Add => {
                r#"<path d="M8 3v10M3 8h10" fill="none" stroke="black" stroke-width="1.6" stroke-linecap="round"/>"#
            }
            Icon::Check => {
                r#"<path d="M3 8.5l3.5 3.5L13 4.5" fill="none" stroke="black" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
            }
            Icon::Close => {
                r#"<path d="M4.5 4.5l7 7M11.5 4.5l-7 7" fill="none" stroke="black" stroke-width="1.6" stroke-linecap="round"/>"#
            }
            Icon::Folder => {
                r#"<path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h2.6a1.5 1.5 0 0 1 1 .4L8 4.2h4.5A1.5 1.5 0 0 1 14 5.7v5.8a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z"/>"#
            }
            Icon::Refresh => {
                r#"<path d="M13 8a5 5 0 1 1-1.46-3.54L13 6M13 2.5V6H9.5" fill="none" stroke="black" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
            }
            Icon::Start => {
                r#"<path d="M5 3.2v9.6a.6.6 0 0 0 .92.5l7.4-4.8a.6.6 0 0 0 0-1l-7.4-4.8a.6.6 0 0 0-.92.5z"/>"#
            }
            Icon::Stop => r#"<rect x="3.5" y="3.5" width="9" height="9" rx="1.5"/>"#,
            Icon::Terminal => {
                r#"<rect x="1.8" y="2.8" width="12.4" height="10.4" rx="2" fill="none" stroke="black" stroke-width="1.6"/><path d="M4.8 6l2 2-2 2M8.5 10.2h2.7" fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>"#
            }
        }
    }

    fn handle(self) -> svg::Handle {
        svg::Handle::from_memory(
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">{}</svg>"#,
                self.body()
            )
            .into_bytes(),
        )
    }

    /// The icon in the color of regular text, dimmed if not `enabled`.
    pub fn view<'a>(self, size: f32, enabled: bool) -> svg::Svg<'a, Theme> {
        // Opacity rather than an alpha in the color, which the tint ignores.
        svg(self.handle())
            .width(size)
            .height(size)
            .opacity(if enabled { 0.9_f32 } else { 0.35 })
            .style(tinted)
    }

    /// The icon of a status page.
    pub fn view_large<'a>(self) -> svg::Svg<'a, Theme> {
        svg(self.handle())
            .width(96)
            .height(96)
            .opacity(0.3_f32)
            .style(tinted)
    }
}

fn tinted(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(colors(theme).fg),
    }
}
