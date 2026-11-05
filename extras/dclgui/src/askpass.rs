//! A password prompt for sudo, which `dockerclaude.sh` needs for mounting.
//!
//! With no terminal to ask on, sudo runs the program in `SUDO_ASKPASS` with the
//! prompt as its argument and reads the password from its stdout. That program
//! is this one, see [`crate::dc::ASKPASS_ENV`].

use iced::theme::Mode;
use iced::widget::{Id, button, column, container, operation, row, text, text_input};
use iced::{Element, Fill, Task, Theme, window};

use crate::style;

struct Askpass {
    prompt: String,
    password: String,
    mode: Mode,
}

#[derive(Debug, Clone)]
enum Message {
    Input(String),
    Submit,
    Cancel,
    ThemeChanged(Mode),
}

fn input_id() -> Id {
    Id::new("password")
}

fn update(state: &mut Askpass, message: Message) {
    match message {
        Message::Input(password) => state.password = password,
        Message::Submit => {
            println!("{}", state.password);
            std::process::exit(0);
        }
        Message::Cancel => std::process::exit(1),
        Message::ThemeChanged(mode) => state.mode = mode,
    }
}

fn view(state: &Askpass) -> Element<'_, Message> {
    let response = |label| button(text(label).width(Fill).center()).width(Fill);

    container(
        column![
            text(&state.prompt).width(Fill).center(),
            text_input("", &state.password)
                .id(input_id())
                .secure(true)
                .padding(8)
                .style(style::entry)
                .on_input(Message::Input)
                .on_submit(Message::Submit),
            row![
                response("Cancel")
                    .style(style::regular)
                    .on_press(Message::Cancel),
                response("OK")
                    .style(style::suggested)
                    .on_press(Message::Submit),
            ]
            .spacing(12),
        ]
        .spacing(14),
    )
    .padding(20)
    .into()
}

/// Shows the prompt, and exits with an error if it is dismissed.
pub fn run(prompt: String) -> iced::Result {
    iced::application(
        move || {
            let state = Askpass {
                prompt: prompt.clone(),
                password: String::new(),
                mode: Mode::None,
            };
            let task = Task::batch([
                operation::focus(input_id()),
                iced::system::theme().map(Message::ThemeChanged),
            ]);
            (state, task)
        },
        update,
        view,
    )
    .title("dclgui: sudo password")
    .theme(|state: &Askpass| -> Theme { style::theme(state.mode) })
    .window(window::Settings {
        size: (420.0, 170.0).into(),
        resizable: false,
        ..window::Settings::default()
    })
    .run()?;

    std::process::exit(1);
}
