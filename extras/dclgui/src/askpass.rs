//! A password prompt for sudo, which `dockerclaude.sh` needs for mounting.
//!
//! With no terminal to ask on, sudo runs the program in `SUDO_ASKPASS` with the
//! prompt as its argument and reads the password from its stdout. That program
//! is this one, see [`crate::dc::ASKPASS_ENV`].

use iced::widget::{Id, button, column, container, operation, row, space, text, text_input};
use iced::{Element, window};

struct Askpass {
    prompt: String,
    password: String,
}

#[derive(Debug, Clone)]
enum Message {
    Input(String),
    Submit,
    Cancel,
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
    }
}

fn view(state: &Askpass) -> Element<'_, Message> {
    container(
        column![
            text(&state.prompt),
            text_input("", &state.password)
                .id(input_id())
                .secure(true)
                .on_input(Message::Input)
                .on_submit(Message::Submit),
            row![
                space::horizontal(),
                button("Cancel")
                    .style(button::secondary)
                    .on_press(Message::Cancel),
                button("OK").on_press(Message::Submit),
            ]
            .spacing(8),
        ]
        .spacing(12),
    )
    .padding(16)
    .into()
}

/// Shows the prompt, and exits with an error if it is dismissed.
pub fn run(prompt: String) -> iced::Result {
    iced::application(
        move || {
            let state = Askpass {
                prompt: prompt.clone(),
                password: String::new(),
            };
            (state, operation::focus(input_id()))
        },
        update,
        view,
    )
    .title("dclgui: sudo password")
    .window(window::Settings {
        size: (420.0, 150.0).into(),
        resizable: false,
        ..window::Settings::default()
    })
    .run()?;

    std::process::exit(1);
}
