//! The main window.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::keyboard::{self, key};
use iced::theme::Mode;
use iced::widget::{
    Id, button, center, column, container, opaque, operation, responsive, row, rule, scrollable,
    space, stack, text, text_input, tooltip,
};
use iced::{
    Center, Element, Event, Fill, FillPortion, Font, Subscription, Task, Theme, font, time,
};

use crate::dc::{self, Invocation, Output};
use crate::parse::{Instance, Issue, Mount, Session, format_mib, truncate};
use crate::style::{self, Icon};

/// `ps -a` is slow, as docker has to work out the container sizes, hence the
/// long period and the Refresh button.
const OVERVIEW_PERIOD: Duration = Duration::from_secs(30);
const DOCTOR_PERIOD: Duration = Duration::from_secs(10);
/// How long a toast stays around, unless it reports an error.
const TOAST_DURATION: Duration = Duration::from_secs(5);

const BODY: u32 = 14;
const CAPTION: u32 = 13;
const ICON: f32 = 16.0;
const MONOSPACE_ADVANCE: f32 = 0.6 * CAPTION as f32;
const LINE_HEIGHT: u32 = 18;
const HEADER_HEIGHT: u32 = 47;
const CONTENT_WIDTH: u32 = 860;
const ROW_PADDING: [u16; 2] = [10, 14];

const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

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
            PromptKind::AddSession => "Add session with mount".to_owned(),
            PromptKind::AddPrefix => "Add prefix".to_owned(),
        }
    }

    fn asks_for_path(&self) -> bool {
        self.kind == PromptKind::AddSession
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

        match &self.kind {
            PromptKind::Rename { old } if !name.is_empty() && name != old => {
                Some(Invocation::new(&self.prefix, ["rename-session", old, name]))
            }
            PromptKind::AddSession if !name.is_empty() && !self.path.is_empty() => {
                Some(mount_invocation(&self.prefix, &self.path, Some(name)))
            }
            PromptKind::AddPrefix => {
                normalize_prefix(name).map(|prefix| Invocation::new(&prefix, ["start-container"]))
            }
            _ => None,
        }
    }
}

fn mount_invocation(prefix: &str, path: &str, session: Option<&str>) -> Invocation {
    let mut args = vec!["mount", path];
    args.extend(session);
    Invocation::new(prefix, args)
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

/// What a directory is being picked for.
#[derive(Debug, Clone)]
enum PickTarget {
    Mount {
        prefix: String,
        session: Option<String>,
    },
    /// The open prompt.
    Prompt,
}

struct Notice {
    id: u64,
    message: String,
    is_error: bool,
}

struct App {
    script: Arc<PathBuf>,
    mode: Mode,
    tab: Tab,
    instances: Vec<Instance>,
    overview_error: Option<String>,
    overview_refresh: Refresh,
    issues: Vec<Issue>,
    doctor_error: Option<String>,
    doctor_refresh: Refresh,
    modal: Option<Modal>,
    /// Whether the file chooser is open.
    picking: bool,
    /// Commands started from the GUI that have not finished yet.
    running: Vec<Invocation>,
    notice: Option<Notice>,
    last_notice_id: u64,
}

#[derive(Debug, Clone)]
enum Message {
    ThemeChanged(Mode),
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
    PickDirectory(PickTarget),
    DirectoryPicked(PickTarget, Result<Option<String>, String>),
    Submit,
    CloseModal,
    FocusNext,
    OpenBash(String),
    DismissNotice,
    ExpireNotice(u64),
}

fn first_input_id() -> Id {
    Id::new("first-input")
}

impl App {
    fn new(script: PathBuf) -> (Self, Task<Message>) {
        let mut app = Self {
            script: Arc::new(script),
            mode: Mode::None,
            tab: Tab::Overview,
            instances: Vec::new(),
            overview_error: None,
            overview_refresh: Refresh::default(),
            issues: Vec::new(),
            doctor_error: None,
            doctor_refresh: Refresh::default(),
            modal: None,
            picking: false,
            running: Vec::new(),
            notice: None,
            last_notice_id: 0,
        };

        let task = Task::batch([
            iced::system::theme().map(Message::ThemeChanged),
            app.refresh_all(),
        ]);
        (app, task)
    }

    fn theme(&self) -> Theme {
        style::theme(self.mode)
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

    /// Shows a toast. Errors stay until dismissed, so as not to be missed.
    fn notify(&mut self, message: String, is_error: bool) -> Task<Message> {
        self.last_notice_id += 1;
        let id = self.last_notice_id;

        self.notice = Some(Notice {
            id,
            message,
            is_error,
        });

        if is_error {
            Task::none()
        } else {
            Task::perform(tokio::time::sleep(TOAST_DURATION), move |()| {
                Message::ExpireNotice(id)
            })
        }
    }

    fn finished(&mut self, command: Invocation, result: Result<Output, String>) -> Task<Message> {
        if let Some(index) = self.running.iter().position(|c| *c == command) {
            self.running.remove(index);
        }

        let notify = match result {
            Ok(output) if output.success => {
                let message = match output.stdout.lines().last() {
                    Some(line) => format!("{command}: {line}"),
                    None => format!("{command}: done"),
                };
                self.notify(message, false)
            }
            Ok(output) => self.notify(
                format!("{command} failed:\n{}", tail(&output.stderr, 6)),
                true,
            ),
            Err(error) => self.notify(error, true),
        };

        Task::batch([notify, self.refresh_all()])
    }

    fn directory_picked(
        &mut self,
        target: PickTarget,
        result: Result<Option<String>, String>,
    ) -> Task<Message> {
        self.picking = false;

        match (result, target) {
            (Err(error), _) => self.notify(error, true),
            (Ok(None), _) => Task::none(),
            (Ok(Some(path)), PickTarget::Mount { prefix, session }) => {
                self.start(mount_invocation(&prefix, &path, session.as_deref()))
            }
            (Ok(Some(path)), PickTarget::Prompt) => {
                if let Some(Modal::Prompt(prompt)) = &mut self.modal {
                    prompt.path = path;
                }
                Task::none()
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ThemeChanged(mode) => {
                self.mode = mode;
                Task::none()
            }
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
            Message::PickDirectory(target) => {
                self.picking = true;
                Task::perform(dc::pick_directory(), move |result| {
                    Message::DirectoryPicked(target.clone(), result)
                })
            }
            Message::DirectoryPicked(target, result) => self.directory_picked(target, result),
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
            Message::OpenBash(prefix) => match dc::open_bash(&self.script, &prefix) {
                Ok(()) => Task::none(),
                Err(error) => self.notify(error, true),
            },
            Message::DismissNotice => {
                self.notice = None;
                Task::none()
            }
            Message::ExpireNotice(id) => {
                if self.notice.as_ref().is_some_and(|notice| notice.id == id) {
                    self.notice = None;
                }
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            time::every(OVERVIEW_PERIOD).map(|_| Message::RefreshOverview),
            time::every(DOCTOR_PERIOD).map(|_| Message::RefreshDoctor),
            iced::system::theme_changes().map(Message::ThemeChanged),
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

        let clamped = container(container(content).max_width(CONTENT_WIDTH))
            .center_x(Fill)
            .padding([20, 16]);

        let page = column![
            self.view_header(),
            rule::horizontal(1).style(style::separator),
            scrollable(clamped)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new()
                        .width(6)
                        .scroller_width(6)
                        .margin(2),
                ))
                .style(style::scrollbar)
                .height(Fill),
        ];

        let mut layers = stack![page].push(self.view_toasts());

        if let Some(modal) = &self.modal {
            layers = layers.push(self.view_modal(modal));
        }

        layers.into()
    }

    /// A header bar with a view switcher.
    fn view_header(&self) -> Element<'_, Message> {
        let tab = |label, tab, badge: Option<Element<'static, Message>>| {
            button(
                row![text(label).size(BODY).font(BOLD)]
                    .push(badge)
                    .spacing(6)
                    .align_y(Center),
            )
            .padding([6, 14])
            .style(style::tab(self.tab == tab))
            .on_press(Message::TabSelected(tab))
        };

        let badge = (!self.issues.is_empty()).then(|| {
            container(text(self.issues.len()).size(11).font(BOLD))
                .padding([0, 6])
                .style(style::badge)
                .into()
        });

        let refresh = with_tooltip(
            button(Icon::Refresh.view(ICON, true))
                .padding(7)
                .style(style::flat)
                .on_press(Message::RefreshAll),
            "Refresh",
        );

        container(
            row![
                container(refresh).width(Fill),
                row![
                    tab("Overview", Tab::Overview, None),
                    tab("Doctor", Tab::Doctor, badge),
                ]
                .spacing(4),
                space::horizontal(),
            ]
            .align_y(Center),
        )
        .height(HEADER_HEIGHT)
        .padding([0, 6])
        .align_y(Center)
        .style(style::header)
        .into()
    }

    /// Commands in progress and the last result, floating above the content.
    fn view_toasts(&self) -> Element<'_, Message> {
        let toast = |content: Element<'static, Message>| {
            container(content)
                .padding([8, 16])
                .max_width(640)
                .style(style::toast)
        };

        let mut toasts = column![].spacing(8).align_x(Center);

        for command in &self.running {
            toasts = toasts.push(toast(
                text(format!("Running {command}…")).size(CAPTION).into(),
            ));
        }

        if let Some(notice) = &self.notice {
            let message = text(notice.message.clone()).size(CAPTION);

            toasts = toasts.push(toast(
                row![
                    if notice.is_error {
                        message.style(style::toast_error)
                    } else {
                        message
                    },
                    button(text("Dismiss").size(CAPTION))
                        .padding([3, 10])
                        .style(style::toast_button)
                        .on_press(Message::DismissNotice),
                ]
                .spacing(12)
                .align_y(Center)
                .into(),
            ));
        }

        container(toasts)
            .center_x(Fill)
            .align_bottom(Fill)
            .padding(16)
            .into()
    }

    fn view_overview(&self) -> Element<'_, Message> {
        if self.instances.is_empty() {
            return match &self.overview_error {
                Some(error) => text(error).style(style::error).into(),
                None => text("Loading…").style(style::dim).into(),
            };
        }

        let mut list = column![].spacing(32).width(Fill);

        if let Some(error) = &self.overview_error {
            list = list.push(text(error).size(CAPTION).style(style::error));
        }

        for instance in &self.instances {
            list = list.push(self.view_instance(instance));
        }

        list.push(
            container(
                pill_with_icon(Icon::Add, "Add prefix", true)
                    .on_press(Message::Prompt(Prompt::new("", PromptKind::AddPrefix))),
            )
            .center_x(Fill),
        )
        .into()
    }

    fn view_instance<'a>(&'a self, instance: &'a Instance) -> Element<'a, Message> {
        let prefix = instance.prefix.as_str();
        let idle = !self.is_busy(prefix);
        let exists = instance.container.is_some();
        let running = instance.container.as_ref().is_some_and(|c| c.running);
        let has_snapshot = instance.snapshot_bytes.is_some();

        let title = row![
            text(format!(
                "Prefix: {}",
                if prefix.is_empty() { "none" } else { prefix }
            ))
            .size(15)
            .font(BOLD)
            .width(Fill),
            pill_with_icon(Icon::Terminal, "Bash", running)
                .on_press_maybe(running.then(|| Message::OpenBash(prefix.to_owned()))),
        ]
        .align_y(Center)
        .padding([0, 4]);

        let start_stop = if running {
            with_tooltip(
                icon_button(
                    Icon::Stop,
                    style::regular_circular,
                    idle.then(|| {
                        Message::Confirm(
                            Confirmation::new(prefix, ["halt-container"])
                                .detail("Sessions running in the container are terminated."),
                        )
                    }),
                ),
                "Stop",
            )
        } else {
            with_tooltip(
                icon_button(
                    Icon::Start,
                    style::regular_circular,
                    idle.then(|| Message::Run(Invocation::new(prefix, ["start-container"]))),
                ),
                "Start",
            )
        };

        let remove_container = with_tooltip(
            icon_button(
                Icon::Close,
                style::flat_circular,
                (idle && exists).then(|| {
                    Message::Confirm(
                        Confirmation::new(prefix, ["stop-container"])
                            .detail("Removes the container. Anything installed in it is lost."),
                    )
                }),
            ),
            "Remove container",
        );

        let container_size = instance.container.as_ref().and_then(|c| c.size_bytes);
        let container_row = action_row(
            row![
                container(space())
                    .width(8)
                    .height(8)
                    .style(style::status_dot(running)),
                text(if running { "Running" } else { "Stopped" }).size(BODY),
            ]
            .spacing(8)
            .align_y(Center),
            size_label("Container", container_size),
            row![start_stop, remove_container]
                .spacing(6)
                .align_y(Center),
        );

        let snapshot = Confirmation::new(prefix, ["snapshot"]);
        let take_snapshot = pill("Snapshot").on_press_maybe((idle && exists).then(|| {
            if has_snapshot {
                Message::Confirm(snapshot.clone().detail("Replaces the existing snapshot."))
            } else {
                Message::Run(snapshot.command.clone())
            }
        }));

        let restore = pill("Restore").on_press_maybe(
            (idle && has_snapshot && !exists)
                .then(|| Message::Run(Invocation::new(prefix, ["restore"]))),
        );
        let restore = if has_snapshot && exists {
            with_tooltip(restore, "Remove the container first")
        } else {
            restore.into()
        };

        let delete_snapshot = with_tooltip(
            icon_button(
                Icon::Close,
                style::flat_circular,
                (idle && has_snapshot).then(|| {
                    Message::Confirm(
                        Confirmation::new(prefix, ["snapshot-delete"])
                            .detail("Deletes the snapshot file and image."),
                    )
                }),
            ),
            "Delete snapshot",
        );

        let snapshot_row = action_row(
            text("Snapshot").size(BODY),
            size_label("Snapshot", instance.snapshot_bytes),
            row![take_snapshot, restore, delete_snapshot]
                .spacing(6)
                .align_y(Center),
        );

        let mut log = column![text("Log").size(BODY)].spacing(3);
        for line in &instance.log {
            log = log.push(one_line(line));
        }
        if instance.log.is_empty() {
            log = log.push(one_line("-"));
        }

        let last_prompt = column![
            text("Last prompt").size(BODY),
            one_line(instance.last_prompt.as_deref().unwrap_or("-")),
        ]
        .spacing(3);

        let details = boxed_list([
            container_row,
            snapshot_row,
            container(log).padding(ROW_PADDING).into(),
            container(last_prompt).padding(ROW_PADDING).into(),
        ]);

        let columns = row![
            text("Session")
                .size(CAPTION)
                .font(BOLD)
                .style(style::dim)
                .width(FillPortion(1)),
            text("Mount(s)")
                .size(CAPTION)
                .font(BOLD)
                .style(style::dim)
                .width(FillPortion(1)),
        ]
        .padding([0, 18]);

        let add_session = container(
            pill_with_icon(Icon::Add, "Add session w/ mount", idle).on_press_maybe(
                idle.then(|| Message::Prompt(Prompt::new(prefix, PromptKind::AddSession))),
            ),
        )
        .padding(ROW_PADDING)
        .into();

        let sessions = boxed_list(
            instance
                .sessions
                .iter()
                .map(|session| self.view_session(prefix, session))
                .chain([add_session]),
        );

        column![title, details, columns, sessions]
            .spacing(10)
            .into()
    }

    fn view_session<'a>(&self, prefix: &'a str, session: &'a Session) -> Element<'a, Message> {
        let idle = !self.is_busy(prefix);

        let name: Element<'_, Message> = match &session.name {
            Some(name) => row![
                column![text(name).size(BODY)]
                    .push(
                        (!session.running)
                            .then(|| { text("Not running").size(CAPTION).style(style::dim) })
                    )
                    .spacing(2)
                    .width(Fill),
                pill("Rename").on_press_maybe(idle.then(|| {
                    Message::Prompt(Prompt::new(
                        prefix,
                        PromptKind::Rename { old: name.clone() },
                    ))
                })),
            ]
            .spacing(8)
            .align_y(Center)
            .into(),
            None => text("No session").size(BODY).style(style::dim).into(),
        };

        let mut mounts = column![].spacing(4);

        for mount in &session.mounts {
            mounts = mounts.push(
                row![
                    text(&mount.label)
                        .size(BODY)
                        .width(Fill)
                        .style(move |theme: &Theme| if mount.broken {
                            style::error(theme)
                        } else {
                            text::Style::default()
                        }),
                    with_tooltip(
                        icon_button(
                            Icon::Close,
                            style::flat_circular,
                            idle.then(|| Message::Confirm(remove_mount(prefix, mount))),
                        ),
                        "Remove mount",
                    ),
                ]
                .spacing(8)
                .align_y(Center),
            );
        }

        let can_pick = idle && !self.picking;

        mounts = mounts.push(
            pill_with_icon(Icon::Add, "Add mount", can_pick).on_press_maybe(can_pick.then(|| {
                Message::PickDirectory(PickTarget::Mount {
                    prefix: prefix.to_owned(),
                    session: session.name.clone(),
                })
            })),
        );

        row![
            container(name).width(FillPortion(1)),
            container(mounts).width(FillPortion(1)),
        ]
        .spacing(16)
        .padding(ROW_PADDING)
        .into()
    }

    fn view_doctor(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(12).width(Fill);

        if let Some(error) = &self.doctor_error {
            list = list.push(text(error).style(style::error));
        }

        if self.issues.is_empty() && self.doctor_error.is_none() {
            return container(
                column![
                    Icon::Check.view_large(),
                    text("No issues found").size(20).font(BOLD),
                ]
                .spacing(12)
                .align_x(Center),
            )
            .center_x(Fill)
            .padding(80)
            .into();
        }

        let rows = self.issues.iter().map(|issue| {
            let fix = issue.fix.as_ref().map(|fix| {
                pill("Fix").on_press_maybe((!self.is_busy(&fix.prefix)).then(|| {
                    Message::Confirm(Confirmation {
                        command: fix.clone(),
                        detail: None,
                    })
                }))
            });

            row![text(&issue.description).size(BODY).width(Fill)]
                .push(fix)
                .spacing(12)
                .align_y(Center)
                .padding(ROW_PADDING)
                .into()
        });

        if !self.issues.is_empty() {
            list = list.push(boxed_list(rows));
        }

        list.into()
    }

    /// An alert dialog.
    fn view_modal<'a>(&self, modal: &'a Modal) -> Element<'a, Message> {
        let heading = |label: String| text(label).size(16).font(BOLD).width(Fill).center();
        let response = |label| {
            button(text(label).size(BODY).width(Fill).center())
                .width(Fill)
                .padding([9, 14])
        };
        let cancel = response("Cancel")
            .style(style::regular)
            .on_press(Message::CloseModal);

        let dialog: Element<'_, Message> = match modal {
            Modal::Confirm(confirmation) => {
                column![heading(format!("Run `{}`?", confirmation.command))]
                    .push(confirmation.detail.as_ref().map(|detail| {
                        text(detail)
                            .size(BODY)
                            .style(style::dim)
                            .width(Fill)
                            .center()
                    }))
                    .push(
                        row![
                            cancel,
                            response("Run")
                                .style(style::destructive)
                                .on_press(Message::Accept),
                        ]
                        .spacing(12),
                    )
                    .spacing(14)
                    .into()
            }
            Modal::Prompt(prompt) => {
                let submit = prompt.invocation().map(|_| Message::Submit);

                let mut form = column![
                    heading(prompt.title()),
                    text_input(prompt.name_placeholder(), &prompt.name)
                        .id(first_input_id())
                        .size(BODY)
                        .padding(9)
                        .style(style::entry)
                        .on_input(Message::NameChanged)
                        .on_submit_maybe(submit.clone()),
                ]
                .spacing(14);

                if prompt.asks_for_path() {
                    let label = if prompt.path.is_empty() {
                        "Choose directory…"
                    } else {
                        prompt.path.as_str()
                    };

                    form = form.push(
                        button(
                            row![
                                Icon::Folder.view(ICON, !self.picking),
                                text(label).size(BODY).width(Fill),
                            ]
                            .spacing(8)
                            .align_y(Center),
                        )
                        .width(Fill)
                        .padding([9, 12])
                        .style(style::regular)
                        .on_press_maybe(
                            (!self.picking).then_some(Message::PickDirectory(PickTarget::Prompt)),
                        ),
                    );
                }

                form.push(
                    row![
                        cancel,
                        response("OK")
                            .style(style::suggested)
                            .on_press_maybe(submit),
                    ]
                    .spacing(12),
                )
                .into()
            }
        };

        opaque(
            center(
                container(dialog)
                    .padding(24)
                    .max_width(460)
                    .style(style::dialog),
            )
            .style(style::scrim),
        )
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

fn pill(label: &str) -> button::Button<'_, Message> {
    button(text(label).size(BODY))
        .padding([5, 12])
        .style(style::regular)
}

fn pill_with_icon<'a>(icon: Icon, label: &'a str, enabled: bool) -> button::Button<'a, Message> {
    button(
        row![icon.view(ICON, enabled), text(label).size(BODY)]
            .spacing(6)
            .align_y(Center),
    )
    .padding([5, 12])
    .style(style::regular)
}

fn icon_button<'a>(
    icon: Icon,
    style: fn(&Theme, button::Status) -> button::Style,
    on_press: Option<Message>,
) -> button::Button<'a, Message> {
    button(icon.view(ICON, on_press.is_some()))
        .padding(6)
        .style(style)
        .on_press_maybe(on_press)
}

fn with_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    tip: &'a str,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(tip).size(CAPTION))
            .padding([5, 10])
            .style(style::tooltip),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

fn size_label(label: &str, bytes: Option<u64>) -> String {
    let size = bytes.map_or("-".to_owned(), format_mib);
    format!("{label} size: {size}")
}

/// A row of a boxed list, with a title and subtitle and something to do on the
/// right.
fn action_row<'a>(
    title: impl Into<Element<'a, Message>>,
    subtitle: String,
    suffix: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        column![title.into(), text(subtitle).size(CAPTION).style(style::dim)]
            .spacing(2)
            .width(Fill),
        suffix.into(),
    ]
    .spacing(12)
    .align_y(Center)
    .padding(ROW_PADDING)
    .into()
}

/// A card of rows with separators between them.
fn boxed_list<'a>(rows: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
    let mut list = column![];

    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            list = list.push(rule::horizontal(1).style(style::separator));
        }
        list = list.push(row);
    }

    container(list).width(Fill).style(style::card).into()
}

/// A line of monospace text, cut off with an ellipsis if it does not fit.
fn one_line(line: &str) -> Element<'_, Message> {
    container(responsive(move |size| {
        let columns = (size.width / MONOSPACE_ADVANCE) as usize;

        text(truncate(line, columns))
            .size(CAPTION)
            .font(Font::MONOSPACE)
            .style(style::dim)
            .wrapping(text::Wrapping::None)
            .into()
    }))
    .height(LINE_HEIGHT)
    .clip(true)
    .into()
}

/// The last `lines` non-empty lines of `output`.
fn tail(output: &str, lines: usize) -> String {
    let all: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

pub fn run(script: PathBuf) -> iced::Result {
    iced::application(move || App::new(script.clone()), App::update, App::view)
        .title("dclgui")
        .theme(App::theme)
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
    fn mounts_are_added_to_their_session_if_any() {
        assert_eq!(
            mount_invocation("work-", "/tmp/a", Some("foo")),
            Invocation::new("work-", ["mount", "/tmp/a", "foo"])
        );
        assert_eq!(
            mount_invocation("", "/tmp/a", None),
            Invocation::new("", ["mount", "/tmp/a"])
        );
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
