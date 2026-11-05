//! The main window.

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::keyboard::{self, key};
use iced::widget::{
    Id, button, center, column, container, opaque, operation, responsive, row, rule, scrollable,
    space, stack, text, text_input, tooltip,
};
use iced::{Center, Color, Element, Event, Fill, FillPortion, Font, Subscription, Task, time};

use crate::dc::{self, Invocation, Output};
use crate::parse::{Instance, Issue, Mount, Session, format_mib, truncate};

/// `ps -a` is slow, as docker has to work out the container sizes, hence the
/// long period and the Refresh button.
const OVERVIEW_PERIOD: Duration = Duration::from_secs(30);
const DOCTOR_PERIOD: Duration = Duration::from_secs(10);

const SMALL: u32 = 13;
const MONOSPACE_ADVANCE: f32 = 0.6 * SMALL as f32;
const LINE_HEIGHT: u32 = 18;
const SIZE_LABEL_WIDTH: u32 = 190;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Overview,
    Doctor,
}

/// Keeps one refresh in flight at a time, and makes sure that one started after
/// the last request, so that the result of an action is not missed.
#[derive(Default)]
struct Refresh {
    running: bool,
    requested: bool,
}

impl Refresh {
    /// Returns whether to start a refresh.
    fn request(&mut self) -> bool {
        self.requested = self.running;
        !std::mem::replace(&mut self.running, true)
    }

    /// Returns whether to start another refresh.
    fn finish(&mut self) -> bool {
        self.running = std::mem::take(&mut self.requested);
        self.running
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PromptKind {
    Rename { old: String },
    AddMount { session: Option<String> },
    AddSession,
    AddPrefix,
}

#[derive(Debug, Clone, PartialEq)]
struct Prompt {
    prefix: String,
    kind: PromptKind,
    name: String,
    path: String,
}

impl Prompt {
    fn new(prefix: &str, kind: PromptKind) -> Self {
        let name = match &kind {
            PromptKind::Rename { old } => old.clone(),
            _ => String::new(),
        };

        Self {
            prefix: prefix.to_owned(),
            kind,
            name,
            path: String::new(),
        }
    }

    fn title(&self) -> String {
        match &self.kind {
            PromptKind::Rename { old } => format!("Rename session {old}"),
            PromptKind::AddMount {
                session: Some(name),
            } => format!("Add mount to session {name}"),
            PromptKind::AddMount { session: None } => "Add mount".to_owned(),
            PromptKind::AddSession => "Add session with mount".to_owned(),
            PromptKind::AddPrefix => "Add prefix".to_owned(),
        }
    }

    fn asks_for_name(&self) -> bool {
        !matches!(self.kind, PromptKind::AddMount { .. })
    }

    fn asks_for_path(&self) -> bool {
        !matches!(self.kind, PromptKind::Rename { .. } | PromptKind::AddPrefix)
    }

    fn name_placeholder(&self) -> &'static str {
        match self.kind {
            PromptKind::AddPrefix => "Prefix, e.g. work-",
            _ => "Session name",
        }
    }

    /// The command to run, or `None` if the input is not complete yet.
    fn invocation(&self) -> Option<Invocation> {
        let name = self.name.trim();
        let path = expand_tilde(self.path.trim());

        if self.kind == PromptKind::AddPrefix {
            return normalize_prefix(name)
                .map(|prefix| Invocation::new(&prefix, ["start-container"]));
        }

        let args = match &self.kind {
            PromptKind::Rename { old } if !name.is_empty() && name != old => {
                vec!["rename-session", old, name]
            }
            PromptKind::AddMount { session } if !path.is_empty() => match session {
                Some(session) => vec!["mount", &path, session],
                None => vec!["mount", &path],
            },
            PromptKind::AddSession if !name.is_empty() && !path.is_empty() => {
                vec!["mount", &path, name]
            }
            _ => return None,
        };

        Some(Invocation::new(&self.prefix, args))
    }
}

/// Makes `prefix` end in a separator, as everything named after it would run
/// into the rest of the name otherwise. Returns `None` if it is not usable as the
/// start of a Compose project name.
fn normalize_prefix(prefix: &str) -> Option<String> {
    let valid_start = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let valid = |c: char| valid_start(c) || c == '-' || c == '_';

    if !prefix.starts_with(valid_start) || !prefix.chars().all(valid) {
        None
    } else if prefix.ends_with(['-', '_']) {
        Some(prefix.to_owned())
    } else {
        Some(format!("{prefix}-"))
    }
}

fn expand_tilde(path: &str) -> String {
    match (path.strip_prefix('~'), env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{home}{rest}")
        }
        _ => path.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Confirmation {
    command: Invocation,
    detail: Option<String>,
}

impl Confirmation {
    fn new<S: Into<String>>(prefix: &str, args: impl IntoIterator<Item = S>) -> Self {
        Self {
            command: Invocation::new(prefix, args),
            detail: None,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

enum Modal {
    Confirm(Confirmation),
    Prompt(Prompt),
}

struct Notice {
    message: String,
    is_error: bool,
}

struct App {
    script: Arc<PathBuf>,
    tab: Tab,
    instances: Vec<Instance>,
    overview_error: Option<String>,
    overview_refresh: Refresh,
    issues: Vec<Issue>,
    doctor_error: Option<String>,
    doctor_refresh: Refresh,
    modal: Option<Modal>,
    /// Commands started from the GUI that have not finished yet.
    running: Vec<Invocation>,
    notice: Option<Notice>,
}

#[derive(Debug, Clone)]
enum Message {
    TabSelected(Tab),
    RefreshOverview,
    RefreshDoctor,
    RefreshAll,
    OverviewLoaded(Result<Vec<Instance>, String>),
    DoctorLoaded(Result<Vec<Issue>, String>),
    Run(Invocation),
    Finished(Invocation, Result<Output, String>),
    Confirm(Confirmation),
    Accept,
    Prompt(Prompt),
    NameChanged(String),
    PathChanged(String),
    Submit,
    CloseModal,
    FocusNext,
    OpenBash(String),
    DismissNotice,
}

fn first_input_id() -> Id {
    Id::new("first-input")
}

impl App {
    fn new(script: PathBuf) -> (Self, Task<Message>) {
        let mut app = Self {
            script: Arc::new(script),
            tab: Tab::Overview,
            instances: Vec::new(),
            overview_error: None,
            overview_refresh: Refresh::default(),
            issues: Vec::new(),
            doctor_error: None,
            doctor_refresh: Refresh::default(),
            modal: None,
            running: Vec::new(),
            notice: None,
        };

        let task = app.refresh_all();
        (app, task)
    }

    fn is_busy(&self, prefix: &str) -> bool {
        self.running.iter().any(|command| command.prefix == prefix)
    }

    fn load_overview(&self) -> Task<Message> {
        Task::perform(
            dc::load_overview(self.script.clone()),
            Message::OverviewLoaded,
        )
    }

    fn load_doctor(&self) -> Task<Message> {
        Task::perform(dc::load_issues(self.script.clone()), Message::DoctorLoaded)
    }

    fn refresh_overview(&mut self) -> Task<Message> {
        if self.overview_refresh.request() {
            self.load_overview()
        } else {
            Task::none()
        }
    }

    fn refresh_doctor(&mut self) -> Task<Message> {
        if self.doctor_refresh.request() {
            self.load_doctor()
        } else {
            Task::none()
        }
    }

    fn refresh_all(&mut self) -> Task<Message> {
        Task::batch([self.refresh_overview(), self.refresh_doctor()])
    }

    fn start(&mut self, command: Invocation) -> Task<Message> {
        self.running.push(command.clone());

        let run = dc::run(self.script.clone(), command.clone());
        Task::perform(run, move |result| {
            Message::Finished(command.clone(), result)
        })
    }

    fn finished(&mut self, command: Invocation, result: Result<Output, String>) -> Task<Message> {
        if let Some(index) = self.running.iter().position(|c| *c == command) {
            self.running.remove(index);
        }

        self.notice = Some(match result {
            Ok(output) if output.success => Notice {
                message: match output.stdout.lines().last() {
                    Some(line) => format!("{command}: {line}"),
                    None => format!("{command}: done"),
                },
                is_error: false,
            },
            Ok(output) => Notice {
                message: format!("{command} failed:\n{}", tail(&output.stderr, 6)),
                is_error: true,
            },
            Err(error) => Notice {
                message: error,
                is_error: true,
            },
        });

        self.refresh_all()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => {
                self.tab = tab;
                Task::none()
            }
            Message::RefreshOverview => self.refresh_overview(),
            Message::RefreshDoctor => self.refresh_doctor(),
            Message::RefreshAll => self.refresh_all(),
            Message::OverviewLoaded(result) => {
                match result {
                    Ok(instances) => {
                        self.instances = instances;
                        self.overview_error = None;
                    }
                    Err(error) => self.overview_error = Some(error),
                }

                if self.overview_refresh.finish() {
                    self.load_overview()
                } else {
                    Task::none()
                }
            }
            Message::DoctorLoaded(result) => {
                match result {
                    Ok(issues) => {
                        self.issues = issues;
                        self.doctor_error = None;
                    }
                    Err(error) => self.doctor_error = Some(error),
                }

                if self.doctor_refresh.finish() {
                    self.load_doctor()
                } else {
                    Task::none()
                }
            }
            Message::Run(command) => self.start(command),
            Message::Finished(command, result) => self.finished(command, result),
            Message::Confirm(confirmation) => {
                self.modal = Some(Modal::Confirm(confirmation));
                Task::none()
            }
            Message::Accept => match self.modal.take() {
                Some(Modal::Confirm(confirmation)) => self.start(confirmation.command),
                modal => {
                    self.modal = modal;
                    Task::none()
                }
            },
            Message::Prompt(prompt) => {
                self.modal = Some(Modal::Prompt(prompt));
                operation::focus(first_input_id())
            }
            Message::NameChanged(name) => {
                if let Some(Modal::Prompt(prompt)) = &mut self.modal {
                    prompt.name = name;
                }
                Task::none()
            }
            Message::PathChanged(path) => {
                if let Some(Modal::Prompt(prompt)) = &mut self.modal {
                    prompt.path = path;
                }
                Task::none()
            }
            Message::Submit => {
                let command = match &self.modal {
                    Some(Modal::Prompt(prompt)) => prompt.invocation(),
                    _ => None,
                };

                match command {
                    Some(command) => {
                        self.modal = None;
                        self.start(command)
                    }
                    None => Task::none(),
                }
            }
            Message::CloseModal => {
                self.modal = None;
                Task::none()
            }
            Message::FocusNext => operation::focus_next(),
            Message::OpenBash(prefix) => {
                if let Err(error) = dc::open_bash(&self.script, &prefix) {
                    self.notice = Some(Notice {
                        message: error,
                        is_error: true,
                    });
                }
                Task::none()
            }
            Message::DismissNotice => {
                self.notice = None;
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            time::every(OVERVIEW_PERIOD).map(|_| Message::RefreshOverview),
            time::every(DOCTOR_PERIOD).map(|_| Message::RefreshDoctor),
        ];

        if self.modal.is_some() {
            subscriptions.push(iced::event::listen_with(|event, _, _| match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => match key {
                    keyboard::Key::Named(key::Named::Escape) => Some(Message::CloseModal),
                    keyboard::Key::Named(key::Named::Tab) => Some(Message::FocusNext),
                    _ => None,
                },
                _ => None,
            }));
        }

        Subscription::batch(subscriptions)
    }

    fn view(&self) -> Element<'_, Message> {
        let content = match self.tab {
            Tab::Overview => self.view_overview(),
            Tab::Doctor => self.view_doctor(),
        };

        let page = column![
            self.view_tabs(),
            rule::horizontal(1),
            container(scrollable(container(content).padding([0, 14])).height(Fill))
                .height(Fill)
                .padding([8, 0]),
            self.view_status(),
        ]
        .spacing(0)
        .padding(8);

        match &self.modal {
            Some(modal) => stack![page, view_modal(modal)].into(),
            None => page.into(),
        }
    }

    fn view_tabs(&self) -> Element<'_, Message> {
        let tab = |label, tab| {
            button(text(label))
                .style(if self.tab == tab {
                    button::primary
                } else {
                    button::text
                })
                .on_press(Message::TabSelected(tab))
        };

        let badge = (!self.issues.is_empty()).then(|| {
            container(text(self.issues.len()).size(11).color(Color::WHITE))
                .padding([1, 6])
                .style(|theme: &iced::Theme| {
                    container::Style::default()
                        .background(theme.palette().danger)
                        .border(iced::border::rounded(9))
                })
        });

        row![tab("Overview", Tab::Overview), tab("Doctor", Tab::Doctor),]
            .push(badge)
            .push(space::horizontal())
            .push(
                button(text("Refresh").size(SMALL))
                    .style(button::secondary)
                    .on_press(Message::RefreshAll),
            )
            .spacing(6)
            .padding([0, 0])
            .align_y(Center)
            .into()
    }

    fn view_status(&self) -> Element<'_, Message> {
        let mut status = column![].spacing(4);

        for command in &self.running {
            status = status.push(text(format!("Running {command}…")).size(SMALL));
        }

        if let Some(notice) = &self.notice {
            status = status.push(
                row![
                    text(&notice.message)
                        .size(SMALL)
                        .width(Fill)
                        .style(if notice.is_error {
                            text::danger
                        } else {
                            text::default
                        }),
                    small_button("Dismiss")
                        .style(button::text)
                        .on_press(Message::DismissNotice),
                ]
                .align_y(Center),
            );
        }

        container(status).padding([4, 14]).into()
    }

    fn view_overview(&self) -> Element<'_, Message> {
        if self.instances.is_empty() {
            return match &self.overview_error {
                Some(error) => text(error).style(text::danger).into(),
                None => text("Loading…").into(),
            };
        }

        let mut list = column![].spacing(12).width(Fill);

        if let Some(error) = &self.overview_error {
            list = list.push(text(error).size(SMALL).style(text::danger));
        }

        for instance in &self.instances {
            list = list.push(self.view_instance(instance));
        }

        list.push(
            small_button("+ Add prefix")
                .on_press(Message::Prompt(Prompt::new("", PromptKind::AddPrefix))),
        )
        .into()
    }

    fn view_instance<'a>(&'a self, instance: &'a Instance) -> Element<'a, Message> {
        let prefix = instance.prefix.as_str();
        let idle = !self.is_busy(prefix);
        let exists = instance.container.is_some();
        let running = instance.container.as_ref().is_some_and(|c| c.running);
        let has_snapshot = instance.snapshot_bytes.is_some();

        let start_stop = if running {
            small_button("⏹️").on_press_maybe(idle.then(|| {
                Message::Confirm(
                    Confirmation::new(prefix, ["halt-container"])
                        .detail("Sessions running in the container are terminated."),
                )
            }))
        } else {
            small_button("▶️").on_press_maybe(
                idle.then(|| Message::Run(Invocation::new(prefix, ["start-container"]))),
            )
        };

        let remove_container = delete_button().on_press_maybe((idle && exists).then(|| {
            Message::Confirm(
                Confirmation::new(prefix, ["stop-container"])
                    .detail("Removes the container. Anything installed in it is lost."),
            )
        }));

        let delete_snapshot = delete_button().on_press_maybe((idle && has_snapshot).then(|| {
            Message::Confirm(
                Confirmation::new(prefix, ["snapshot-delete"])
                    .detail("Deletes the snapshot file and image."),
            )
        }));

        let snapshot = Confirmation::new(prefix, ["snapshot"]);
        let take_snapshot = small_button("Snapshot").on_press_maybe((idle && exists).then(|| {
            if has_snapshot {
                Message::Confirm(snapshot.clone().detail("Replaces the existing snapshot."))
            } else {
                Message::Run(snapshot.command.clone())
            }
        }));

        let restore = small_button("Restore").on_press_maybe(
            (idle && has_snapshot && !exists)
                .then(|| Message::Run(Invocation::new(prefix, ["restore"]))),
        );
        let restore: Element<'_, Message> = if has_snapshot && exists {
            tooltip(
                restore,
                container(text("Remove the container first").size(SMALL))
                    .padding(6)
                    .style(container::rounded_box),
                tooltip::Position::Bottom,
            )
            .into()
        } else {
            restore.into()
        };

        let size_label = |label: &str, bytes: Option<u64>| {
            let size = bytes.map_or("-".to_owned(), format_mib);
            text(format!("{label} size: {size}"))
                .size(SMALL)
                .width(SIZE_LABEL_WIDTH)
        };

        let container_size = instance.container.as_ref().and_then(|c| c.size_bytes);
        let header = column![
            row![
                text(format!(
                    "Prefix: {}",
                    if prefix.is_empty() { "none" } else { prefix }
                ))
                .font(Font {
                    weight: iced::font::Weight::Bold,
                    ..Font::DEFAULT
                }),
                space::horizontal(),
                start_stop,
                text(if running { "Running" } else { "Stopped" })
                    .size(SMALL)
                    .style(if running {
                        text::success
                    } else {
                        text::secondary
                    }),
                remove_container,
                size_label("Container", container_size),
            ]
            .spacing(8)
            .align_y(Center),
            row![
                small_button("Bash")
                    .on_press_maybe((running).then(|| Message::OpenBash(prefix.to_owned()))),
                space::horizontal(),
                delete_snapshot,
                size_label("Snapshot", instance.snapshot_bytes),
            ]
            .spacing(8)
            .align_y(Center),
            row![space::horizontal(), take_snapshot, restore].spacing(8),
        ]
        .spacing(4);

        let mut log = column![text("Log:").size(SMALL)].spacing(2);
        for line in &instance.log {
            log = log.push(one_line(line));
        }
        if instance.log.is_empty() {
            log = log.push(one_line("-"));
        }

        let last_prompt = column![
            text("Last prompt:").size(SMALL),
            one_line(instance.last_prompt.as_deref().unwrap_or("-")),
        ]
        .spacing(2);

        let mut sessions = column![
            row![
                text("Session").size(SMALL).width(FillPortion(1)),
                text("Mount(s)").size(SMALL).width(FillPortion(1)),
            ],
            rule::horizontal(1),
        ]
        .spacing(4);

        for session in &instance.sessions {
            sessions = sessions
                .push(self.view_session(prefix, session))
                .push(rule::horizontal(1));
        }

        sessions = sessions.push(small_button("+ Add session w/ mount").on_press_maybe(
            idle.then(|| Message::Prompt(Prompt::new(prefix, PromptKind::AddSession))),
        ));

        container(column![header, log, last_prompt, sessions].spacing(10))
            .padding(10)
            .width(Fill)
            .style(container::bordered_box)
            .into()
    }

    fn view_session<'a>(&self, prefix: &'a str, session: &'a Session) -> Element<'a, Message> {
        let idle = !self.is_busy(prefix);

        let name: Element<'_, Message> = match &session.name {
            Some(name) => row![
                small_button("Rename").on_press_maybe(idle.then(|| {
                    Message::Prompt(Prompt::new(
                        prefix,
                        PromptKind::Rename { old: name.clone() },
                    ))
                })),
                text(name).size(SMALL),
            ]
            .push(
                (!session.running)
                    .then(|| text("(not running)").size(SMALL).style(text::secondary)),
            )
            .spacing(8)
            .align_y(Center)
            .into(),
            None => text("(no session)")
                .size(SMALL)
                .style(text::secondary)
                .into(),
        };

        let mut mounts = column![].spacing(4);

        for mount in &session.mounts {
            mounts = mounts.push(
                row![
                    delete_button().on_press_maybe(
                        idle.then(|| Message::Confirm(remove_mount(prefix, mount)))
                    ),
                    text(&mount.label).size(SMALL).style(if mount.broken {
                        text::danger
                    } else {
                        text::default
                    }),
                ]
                .spacing(8)
                .align_y(Center),
            );
        }

        mounts = mounts.push(small_button("+ Add mount").on_press_maybe(idle.then(|| {
            Message::Prompt(Prompt::new(
                prefix,
                PromptKind::AddMount {
                    session: session.name.clone(),
                },
            ))
        })));

        row![
            container(name).width(FillPortion(1)),
            container(mounts).width(FillPortion(1)),
        ]
        .into()
    }

    fn view_doctor(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(6).width(Fill);

        if let Some(error) = &self.doctor_error {
            list = list.push(text(error).style(text::danger));
        }

        if self.issues.is_empty() && self.doctor_error.is_none() {
            list = list.push(text("No issues found"));
        }

        for issue in &self.issues {
            let fix = issue.fix.as_ref().map(|fix| {
                small_button("fix").on_press_maybe((!self.is_busy(&fix.prefix)).then(|| {
                    Message::Confirm(Confirmation {
                        command: fix.clone(),
                        detail: None,
                    })
                }))
            });

            list = list
                .push(
                    row![text(&issue.description).size(SMALL).width(Fill)]
                        .push(fix)
                        .spacing(8)
                        .align_y(Center),
                )
                .push(rule::horizontal(1));
        }

        list.into()
    }
}

/// What the `x` button of a mount does: plain `umount` needs the host directory,
/// which is not known for a broken mount.
fn remove_mount(prefix: &str, mount: &Mount) -> Confirmation {
    if mount.broken {
        Confirmation::new(prefix, ["force-clean-mounts", &mount.slug])
    } else if mount.refcount > 1 {
        Confirmation::new(prefix, ["umount", &mount.label]).detail(format!(
            "Drops one of {} references, it stays mounted until the last one is gone.",
            mount.refcount
        ))
    } else {
        Confirmation::new(prefix, ["umount", &mount.label])
    }
}

fn small_button(label: &str) -> button::Button<'_, Message> {
    button(text(label).size(SMALL)).padding([2, 8])
}

fn delete_button<'a>() -> button::Button<'a, Message> {
    small_button("x").style(button::danger)
}

/// A line of monospace text, cut off with an ellipsis if it does not fit.
fn one_line(line: &str) -> Element<'_, Message> {
    container(responsive(move |size| {
        let columns = (size.width / MONOSPACE_ADVANCE) as usize;

        text(truncate(line, columns))
            .size(SMALL)
            .font(Font::MONOSPACE)
            .wrapping(text::Wrapping::None)
            .into()
    }))
    .padding(iced::Padding::ZERO.left(12))
    .height(LINE_HEIGHT)
    .clip(true)
    .into()
}

/// The last `lines` non-empty lines of `output`.
fn tail(output: &str, lines: usize) -> String {
    let all: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn view_modal(modal: &Modal) -> Element<'_, Message> {
    let cancel = button("Cancel")
        .style(button::secondary)
        .on_press(Message::CloseModal);

    let dialog: Element<'_, Message> = match modal {
        Modal::Confirm(confirmation) => column![text(format!("Run `{}`?", confirmation.command))]
            .push(
                confirmation
                    .detail
                    .as_ref()
                    .map(|detail| text(detail).size(SMALL).style(text::secondary)),
            )
            .push(
                row![
                    space::horizontal(),
                    cancel,
                    button("Run")
                        .style(button::danger)
                        .on_press(Message::Accept),
                ]
                .spacing(8),
            )
            .spacing(12)
            .into(),
        Modal::Prompt(prompt) => {
            let submit = prompt.invocation().map(|_| Message::Submit);
            let mut form = column![text(prompt.title())].spacing(12);

            if prompt.asks_for_name() {
                form = form.push(
                    text_input(prompt.name_placeholder(), &prompt.name)
                        .id(first_input_id())
                        .on_input(Message::NameChanged)
                        .on_submit_maybe(submit.clone()),
                );
            }

            if prompt.asks_for_path() {
                let input = text_input("Absolute path of a directory", &prompt.path)
                    .on_input(Message::PathChanged)
                    .on_submit_maybe(submit.clone());

                form = form.push(if prompt.asks_for_name() {
                    input
                } else {
                    input.id(first_input_id())
                });
            }

            form.push(
                row![
                    space::horizontal(),
                    cancel,
                    button("OK").on_press_maybe(submit),
                ]
                .spacing(8),
            )
            .into()
        }
    };

    opaque(
        center(
            container(dialog)
                .padding(16)
                .max_width(560)
                .style(container::bordered_box),
        )
        .style(|_| {
            container::Style::default().background(Color {
                a: 0.6,
                ..Color::BLACK
            })
        }),
    )
}

pub fn run(script: PathBuf) -> iced::Result {
    iced::application(move || App::new(script.clone()), App::update, App::view)
        .title("dclgui")
        .subscription(App::subscription)
        .window_size((980.0, 720.0))
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(prompt: &Prompt) -> Option<Vec<String>> {
        prompt.invocation().map(|invocation| invocation.args)
    }

    #[test]
    fn refresh_coalesces_requests() {
        let mut refresh = Refresh::default();

        assert!(refresh.request());
        assert!(!refresh.request());
        assert!(!refresh.request());
        assert!(refresh.finish());
        assert!(!refresh.finish());
        assert!(refresh.request());
    }

    #[test]
    fn rename_prompt_needs_a_new_name() {
        let kind = PromptKind::Rename {
            old: "old".to_owned(),
        };
        let mut prompt = Prompt::new("work-", kind);

        assert_eq!(args(&prompt), None);

        prompt.name = " new name ".to_owned();
        assert_eq!(
            args(&prompt).unwrap(),
            ["rename-session", "old", "new name"]
        );
    }

    #[test]
    fn add_mount_prompt_passes_the_session_if_any() {
        let session = Some("foo".to_owned());
        let mut prompt = Prompt::new("", PromptKind::AddMount { session });

        assert_eq!(args(&prompt), None);

        prompt.path = "/tmp/a".to_owned();
        assert_eq!(args(&prompt).unwrap(), ["mount", "/tmp/a", "foo"]);

        prompt.kind = PromptKind::AddMount { session: None };
        assert_eq!(args(&prompt).unwrap(), ["mount", "/tmp/a"]);
    }

    #[test]
    fn add_session_prompt_needs_name_and_path() {
        let mut prompt = Prompt::new("", PromptKind::AddSession);

        prompt.name = "foo".to_owned();
        assert_eq!(args(&prompt), None);

        prompt.path = "/tmp/a".to_owned();
        assert_eq!(args(&prompt).unwrap(), ["mount", "/tmp/a", "foo"]);
    }

    #[test]
    fn add_prefix_prompt_starts_a_container_for_the_prefix() {
        let mut prompt = Prompt::new("", PromptKind::AddPrefix);

        assert_eq!(prompt.invocation(), None);

        prompt.name = "work".to_owned();
        assert_eq!(
            prompt.invocation(),
            Some(Invocation::new("work-", ["start-container"]))
        );
    }

    #[test]
    fn prefixes_are_normalized() {
        assert_eq!(normalize_prefix("work-").as_deref(), Some("work-"));
        assert_eq!(normalize_prefix("work_").as_deref(), Some("work_"));
        assert_eq!(
            normalize_prefix("fras-dbdig").as_deref(),
            Some("fras-dbdig-")
        );
        assert_eq!(normalize_prefix("Work"), None);
        assert_eq!(normalize_prefix("-work"), None);
        assert_eq!(normalize_prefix("a b"), None);
        assert_eq!(normalize_prefix(""), None);
    }

    #[test]
    fn removing_a_broken_mount_goes_by_slug() {
        let mut mount = Mount {
            label: "/tmp/a".to_owned(),
            slug: "-tmp-a".to_owned(),
            refcount: 1,
            broken: false,
        };

        assert_eq!(remove_mount("", &mount).command.args, ["umount", "/tmp/a"]);

        mount.broken = true;
        assert_eq!(
            remove_mount("", &mount).command.args,
            ["force-clean-mounts", "-tmp-a"]
        );
    }

    #[test]
    fn tail_skips_blank_lines() {
        assert_eq!(tail("a\n\nb\n  \nc\n", 2), "b\nc");
        assert_eq!(tail("", 2), "");
    }
}
