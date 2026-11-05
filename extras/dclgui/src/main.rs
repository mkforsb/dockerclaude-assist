//! GUI frontend for `dockerclaude.sh`. All data and functionality comes from
//! running that script and parsing its output.

mod app;
mod askpass;
mod dc;
mod parse;
mod style;

fn main() -> iced::Result {
    if std::env::var_os(dc::ASKPASS_ENV).is_some() {
        return askpass::run(std::env::args().nth(1).unwrap_or_default());
    }

    let Some(script) = dc::find_script() else {
        eprintln!("dclgui: dockerclaude.sh not found, set DCLGUI_SCRIPT to its path");
        std::process::exit(1);
    };

    dc::watch_for_picker_failures();

    app::run(script)
}
