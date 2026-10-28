//! The native multi-instance control plane.
//!
//! One TUI owns one public loopback port and one subprocess per running instance.
//! The subprocesses keep their endpoints private — see [`super::transport`]; this
//! process routes raw HTTP connections by `Host`, so ordinary requests and
//! WebSocket upgrades follow exactly the same path without reimplementing either
//! protocol.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use futures_util::StreamExt as _;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

use super::access::{Access, LOGIN_COOKIE_PREFIX, MAX_WRONG, PinState, pin_problem};
use super::{Handshake, transport};
use crate::config::{
    Audience, ConfigFile, DEFAULT_BRANDING, DEFAULT_SIZE, Protocol, TargetConfig,
};
use crate::vault::{Key, MasterPassword};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_GRACE: Duration = Duration::from_millis(1500);
const REQUEST_HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST_HEAD: usize = 64 * 1024;
pub(super) const MASTER_HOST: &str = "zdesk.localhost";
/// The binary's version, so the header answers what `--version` would without
/// leaving the TUI. Every instance is a worker of this same binary.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A first launch's complete instance config: no server block and no pretend
/// target. The TUI creates this atomically before adding the instance to its list.
pub const INSTANCE_TEMPLATE: &str = r#"# A zdesk local instance.
#
# There is no [server] block. The TUI control plane owns the shared loopback
# listener, this instance's subdomain, its private endpoint, and its launch
# token. Only [branding] and [[targets]] belong here.

# [branding]
# text = "zDesk"
# logo = "/path/to/logo.png"

# A target names its password; [passwords] keeps it encrypted under the
# master password in your keychain. Press p in the TUI to add one.

# [[targets]]
# name = "work"
# protocol = "rdp"
# host = "192.168.1.20"
# username = "andrew"
# password_ref = "work"
# domain = "CORP"

# [[targets]]
# name = "pi"
# protocol = "vnc"
# host = "192.168.1.30"
# vnc_password_ref = "pi"
"#;

/// Inputs resolved by the CLI before terminal state is changed.
#[derive(Clone, Debug)]
pub struct TuiOptions {
    pub port: u16,
    pub instances_dir: PathBuf,
}

/// The platform's private application-data directory for local instances.
pub fn default_instances_dir() -> anyhow::Result<PathBuf> {
    // Local rather than roaming: the configs hold credentials, and a roaming
    // profile would copy them to every machine the account signs in to.
    #[cfg(windows)]
    {
        let data = std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .context("LOCALAPPDATA is not set; pass --instances-dir")?;
        Ok(PathBuf::from(data).join("zdesk").join("instances"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::home_dir().context("cannot find the home directory; pass --instances-dir")?;
        Ok(home.join("Library/Application Support/zdesk/instances"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(data).join("zdesk/instances"));
        }
        let home = std::env::home_dir().context("cannot find the home directory; pass --instances-dir")?;
        Ok(home.join(".local/share/zdesk/instances"))
    }
}

/// Run the terminal UI and its shared-port router until `q` or a shutdown signal.
pub async fn run_tui(options: TuiOptions) -> anyhow::Result<()> {
    let mocked = crate::keychain::mocked()?;
    let mut master = Master::default();
    let keychain = master.get().map(|_| ());
    let binary = std::env::current_exe().context("cannot locate the zdesk executable")?;
    let mut supervisor = Supervisor::open(options.instances_dir.clone(), binary).await?;
    let access = Access::open(options.port);
    let router = SharedPort::bind(options.port, supervisor.routes(), access.clone()).await?;

    let mut terminal = TerminalSession::enter()?;
    let mut screen = Screen::default();
    let mut events = EventStream::new();
    let mut ticks = tokio::time::interval(Duration::from_millis(250));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut interrupt = Box::pin(tokio::signal::ctrl_c());
    let mut selected = 0usize;
    let mut view = View::List;
    let mut message = match keychain {
        Err(error) => format!("{error:#}; an instance whose config names passwords cannot start until it answers"),
        Ok(()) if mocked => format!(
            "{}=mock: passwords set now open only until the TUI quits",
            crate::keychain::MOCK_ENV
        ),
        Ok(()) => format!("control plane listening on http://{MASTER_HOST}:{}", router.port()),
    };

    loop {
        let instances = supervisor.instances();
        selected = selected.min(instances.len().saturating_sub(1));
        let size = terminal::size().context("cannot read terminal size")?;
        screen.draw(
            render(size, &options, &access.pin(), &instances, selected, &view, &message)?,
            &mut std::io::stdout().lock(),
        )?;

        tokio::select! {
            _ = ticks.tick() => {
                if let Some(ended) = supervisor.poll_exits().await? {
                    message = ended;
                }
            }
            result = &mut interrupt => {
                result.context("cannot listen for Ctrl+C")?;
                break;
            }
            event = events.next() => {
                let Some(event) = event else {
                    anyhow::bail!("terminal input ended");
                };
                let event = event.context("cannot read terminal input")?;
                let Event::Key(key) = event else { continue };
                if key.kind != KeyEventKind::Press { continue; }

                if let View::Naming(name) = &mut view {
                    match key.code {
                        KeyCode::Esc => {
                            view = View::List;
                            message = "instance creation cancelled".to_owned();
                        }
                        KeyCode::Backspace => {
                            name.pop();
                        }
                        KeyCode::Char(character)
                            if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            name.push(character);
                        }
                        KeyCode::Enter => {
                            let requested = std::mem::take(name);
                            match supervisor.create(&requested).await {
                                Ok(index) => {
                                    selected = index;
                                    view = View::List;
                                    message = format!("created {requested}; edit its config, then start it");
                                }
                                Err(error) => message = format!("cannot create instance: {error:#}"),
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                // A page is a reader, so it takes only the keys that move within
                // it or leave it: acting on an instance is the list's, and a stop
                // pressed over a page of text is one nobody aimed.
                if let View::Page { lines, offset, .. } = &mut view {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                        KeyCode::Up | KeyCode::Char('k') => *offset = offset.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('j') => {
                            *offset = (*offset + 1).min(lines.len().saturating_sub(1));
                        }
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?') => view = View::List,
                        _ => {}
                    }
                    continue;
                }

                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                let Some(action) = action_for(key.code) else { continue };
                match action {
                    Action::Quit => break,
                    Action::Up => selected = selected.saturating_sub(1),
                    Action::Down => {
                        selected = (selected + 1).min(instances.len().saturating_sub(1));
                    }
                    Action::NewPin => {
                        access.draw();
                        message = match access.pin() {
                            PinState::Open { expires_at, .. } => format!("new PIN; it expires at {expires_at}"),
                            _ => "new PIN".to_owned(),
                        };
                    }
                    Action::CopyPin => {
                        message = match access.pin() {
                            PinState::Open { digits, .. } => match copy_pin(&dashed(&digits)).await {
                                Ok(()) => UNCONFIRMED_COPY.to_owned(),
                                Err(error) => format!("cannot copy the PIN: {error:#}"),
                            },
                            _ => "no PIN is open; press P for a new one".to_owned(),
                        };
                    }
                    Action::Keys => {
                        view = View::Page {
                            title: "keys".to_owned(),
                            lines: key_lines(),
                            offset: 0,
                        };
                    }
                    Action::New => view = View::Naming(String::new()),
                    Action::Rescan => {
                        supervisor.rescan().await?;
                        message = "rescanned the instances directory".to_owned();
                    }
                    Action::StartAll => {
                        let stopped: Vec<_> = supervisor
                            .instances()
                            .into_iter()
                            .filter(|i| !matches!(i.status, InstanceStatus::Running | InstanceStatus::Starting))
                            .collect();
                        let mut failed = false;
                        for instance in stopped {
                            let name = instance.name.clone();
                            let started = match master.key_for(&instance.config_path()) {
                                Ok(key) => supervisor.start(&name, key.as_ref()).await,
                                Err(error) => Err(error),
                            };
                            if let Err(error) = started {
                                message = format!("{name}: {error:#}");
                                failed = true;
                            }
                        }
                        if !failed {
                            message = "started every stopped instance".to_owned();
                        }
                    }
                    Action::Start => {
                        if let Some(instance) = instances.get(selected) {
                            let name = instance.name.clone();
                            message = if instance.status == InstanceStatus::Running {
                                format!("{name} is already running; x stops it")
                            } else {
                                let started = match master.key_for(&instance.config_path()) {
                                    Ok(key) => supervisor.start(&name, key.as_ref()).await,
                                    Err(error) => Err(error),
                                };
                                match started {
                                    Ok(()) => format!("started {name}"),
                                    Err(error) => format!("{name}: {error:#}"),
                                }
                            };
                        }
                    }
                    Action::Specs => {
                        if let Some(instance) = instances.get(selected) {
                            view = View::Page {
                                title: format!("instance {}", instance.name),
                                lines: describe_instance(instance, router.port()),
                                offset: 0,
                            };
                        }
                    }
                    Action::Stop => {
                        if let Some(instance) = instances.get(selected) {
                            let name = instance.name.clone();
                            message = match supervisor.stop(&name).await {
                                Ok(()) => format!("stopped {name}"),
                                Err(error) => format!("{name}: {error:#}"),
                            };
                        }
                    }
                    Action::Restart => {
                        if let Some(instance) = instances.get(selected) {
                            let name = instance.name.clone();
                            let restarted = match master.key_for(&instance.config_path()) {
                                Ok(key) => supervisor.restart(&name, key.as_ref()).await,
                                Err(error) => Err(error),
                            };
                            message = match restarted {
                                Ok(()) => format!("restarted {name}"),
                                Err(error) => format!("{name}: {error:#}"),
                            };
                        }
                    }
                    Action::Open => {
                        if let Some(instance) = instances.get(selected) {
                            let url = instance_url(&instance.name, router.port());
                            message = if instance.status != InstanceStatus::Running {
                                format!("{} is {}; s starts it", instance.name, instance.status.label())
                            } else {
                                match open_browser(&url) {
                                    Ok(()) => format!("opening {url}"),
                                    Err(error) => format!("cannot open {url}: {error:#}"),
                                }
                            };
                        }
                    }
                    Action::Password => {
                        if let Some(instance) = instances.get(selected) {
                            let name = instance.name.clone();
                            terminal.suspend()?;
                            let set = master.set_password(&name, &instance.config_path());
                            terminal.resume()?;
                            screen.invalidate();
                            events = EventStream::new();
                            message = match set {
                                Ok(password) => format!(
                                    "{name}: [passwords].{password} set; name it in a target's \
                                     password_ref (vnc_password_ref for a VNC server's own \
                                     password), then restart {name}"
                                ),
                                Err(error) => format!("{name}: {error:#}"),
                            };
                        }
                    }
                    Action::Edit => {
                        if let Some(instance) = instances.get(selected) {
                            let name = instance.name.clone();
                            let path = instance.config_path();
                            terminal.suspend()?;
                            let edited = edit_config(&path).await;
                            terminal.resume()?;
                            screen.invalidate();
                            events = EventStream::new();
                            message = match edited.and_then(|()| validate_config(&path)) {
                                Ok(()) => format!("{name} config is valid; restart it to apply changes"),
                                Err(error) => format!("{name} config: {error:#}"),
                            };
                        }
                    }
                }
            }
        }
    }

    terminal.suspend()?;
    supervisor.shutdown().await;
    drop(router);
    println!("zdesk: every instance stopped");
    Ok(())
}

/// The master password this control plane opens its instances' configs with:
/// the account's, kept in the keychain.
///
/// Generated the first time the account needs one and kept there from then on,
/// as Chrome's and Slack's Safe Storage keys are, so nobody types or knows it
/// and every instance opens under the same one. Never asked for at the terminal,
/// and never `[server].master_password_file`: an instance has no `[server]`
/// block. Read from the keychain once per run, when the TUI starts and before
/// it takes the screen, so a keychain that asks the user with a system prompt
/// asks then rather than at the first start of an instance; a read that failed
/// is tried again by the next thing that needs it.
#[derive(Default)]
struct Master(Option<MasterPassword>);

impl Master {
    /// The keychain's master password, generated and kept there when it keeps
    /// none. A keychain that would not answer is the error, so the TUI says so.
    fn get(&mut self) -> anyhow::Result<&MasterPassword> {
        if self.0.is_none() {
            let _one_at_a_time = Self::lock()?;
            let master = match crate::keychain::load().context("cannot read the master password")? {
                Some(master) => master,
                None => {
                    let master = MasterPassword::generate();
                    crate::keychain::save(&master).context("cannot keep a new master password")?;
                    master
                }
            };
            self.0 = Some(master);
        }
        Ok(self.0.as_ref().expect("set just above"))
    }

    /// The account's lock on first-time generation, so two control planes
    /// started together (each with its own port and instances directory) do not
    /// both find the keychain empty and each keep a master the other's save
    /// replaces. The keychain has no create-if-absent, so a file beside the
    /// account's default instances directory stands in, whichever directory
    /// either was given. On the mock store, under test or `ZDESK_KEYCHAIN=mock`,
    /// the store is the process's own and there is nothing to share.
    fn lock() -> anyhow::Result<Option<std::fs::File>> {
        if crate::keychain::mocked()? {
            return Ok(None);
        }
        let instances = default_instances_dir()?;
        let root = instances.parent().context("the instances directory has no parent")?;
        std::fs::create_dir_all(root).with_context(|| format!("cannot create {}", root.display()))?;
        let path = root.join("master.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        file.lock().with_context(|| format!("cannot lock {}", path.display()))?;
        Ok(Some(file))
    }

    /// The key a start of the instance whose config is at `path` hands its worker:
    /// `None` for a config that names no passwords. A `[master]` block the
    /// keychain's master password does not open was made under another account's,
    /// and is told as one.
    fn key_for(&mut self, path: &Path) -> anyhow::Result<Option<Key>> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let file = ConfigFile::parse_with(&text, Audience::Worker)?;
        if !file.needs_master() {
            return Ok(None);
        }
        let master = self.get()?;
        file.key(master).map(Some).with_context(|| foreign_master(path))
    }

    /// Ask for a password's name and the password, on a terminal already
    /// suspended, and encrypt it into the config at `path` — giving the config its
    /// `[master]` block first when it has none. Returns the name.
    fn set_password(&mut self, instance: &str, path: &Path) -> anyhow::Result<String> {
        print!("Password name in {instance} (what a target's password_ref or vnc_password_ref says): ");
        std::io::stdout().flush()?;
        let mut name = String::new();
        std::io::stdin().read_line(&mut name).context("cannot read the name")?;
        let name = name.trim().to_owned();
        crate::vault::valid_name(&name)?;

        let master = self.get()?.clone();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let has_master = text
            .parse::<toml::Table>()
            .with_context(|| format!("{} is not valid TOML", path.display()))?
            .contains_key("master");
        if has_master {
            Key::derive(&master, &crate::vault::master_of(path)?).with_context(|| foreign_master(path))?;
        } else {
            crate::vault::init_file(path, &master)?;
        }
        let password = rpassword::prompt_password(format!("Password for {name}: "))
            .context("cannot read the password")?;
        let confirm = rpassword::prompt_password(format!("Confirm password for {name}: "))
            .context("cannot read the password")?;
        anyhow::ensure!(password == confirm, "the passwords do not match");
        crate::vault::set_file(path, &name, &password, &master)?;
        Ok(name)
    }
}

/// What a config whose `[master]` the keychain's master password does not open
/// is told: it was made under another account's, and nothing here can derive
/// that key again.
fn foreign_master(path: &Path) -> String {
    format!(
        "{} was encrypted under another account's master password: remove its [master] and \
         [passwords] and press p to set each password again",
        path.display()
    )
}

/// Opens the instance config in the operator's editor and waits for it.
///
/// `VISUAL` and `EDITOR` hold a shell command line, not a program path: `subl -w`
/// and `code --wait` are ordinary values, and every tool that honours these
/// variables lets the shell split them. So the value is handed to the platform's
/// shell with the path kept out of the words it splits.
async fn edit_config(path: &Path) -> anyhow::Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| String::from(DEFAULT_EDITOR));
    let status = editor_command(&editor, path)
        .status()
        .await
        .with_context(|| format!("cannot start editor {editor}"))?;
    anyhow::ensure!(status.success(), "editor {editor} exited with {status}");
    Ok(())
}

#[cfg(unix)]
const DEFAULT_EDITOR: &str = "vi";
#[cfg(windows)]
const DEFAULT_EDITOR: &str = "notepad";

/// `sh -c`, with the path as a positional argument.
#[cfg(unix)]
fn editor_command(editor: &str, path: &Path) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(format!("{editor} \"$@\"")).arg("sh").arg(path);
    command
}

/// `cmd /c`, with the path quoted after the editor's own words.
///
/// The line is handed over raw, because cmd does not split its command line the
/// way a program's `argv` is split and an escaped argument would reach it with
/// its escapes. `/s` strips exactly the outer pair of quotes, leaving
/// `<editor> "%ZDESK_EDIT_PATH%"`. The path arrives in that variable rather than
/// in the line, because cmd expands `%NAME%` inside quotes too and a directory may
/// be called `%TEMP%`; what a variable expands to is not expanded again, and a
/// Windows path cannot hold a `"` to break out of its quotes. `/v:off` keeps a `!`
/// literal whatever the registry says about delayed expansion, and `/d` skips any
/// AutoRun it sets.
#[cfg(windows)]
fn editor_command(editor: &str, path: &Path) -> Command {
    let mut command = Command::new("cmd.exe");
    command
        .env("ZDESK_EDIT_PATH", path)
        .raw_arg(format!("/d /v:off /s /c \"{editor} \"%ZDESK_EDIT_PATH%\"\""));
    command
}

/// The instance's origin on the shared port. A browser without a login there is
/// asked for the PIN on it.
fn instance_url(name: &str, port: u16) -> String {
    format!("http://{name}.{MASTER_HOST}:{port}")
}

/// Hands `url` to the desktop's default browser without waiting for it: some
/// launchers stay until the browser exits. None of them gets the terminal, which
/// the TUI is drawing on.
fn open_browser(url: &str) -> anyhow::Result<()> {
    browser_command(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot start the browser launcher")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

/// `start`, whose first quoted word is a window title, so the empty one keeps
/// the URL from being taken for it.
#[cfg(windows)]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("cmd.exe");
    command.raw_arg(format!("/d /c start \"\" \"{url}\""));
    command
}

fn validate_config(path: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    super::check(&text)
}

struct TerminalSession {
    active: bool,
}

impl TerminalSession {
    fn enter() -> anyhow::Result<Self> {
        anyhow::ensure!(std::io::IsTerminal::is_terminal(&std::io::stdin()), "the TUI needs a terminal");
        anyhow::ensure!(std::io::IsTerminal::is_terminal(&std::io::stdout()), "the TUI needs a terminal");
        terminal::enable_raw_mode().context("cannot enable terminal raw mode")?;
        if let Err(error) = execute!(std::io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("cannot enter the alternate screen");
        }
        Ok(Self { active: true })
    }

    fn suspend(&mut self) -> anyhow::Result<()> {
        if self.active {
            execute!(std::io::stdout(), Show, LeaveAlternateScreen)
                .context("cannot leave the alternate screen")?;
            terminal::disable_raw_mode().context("cannot restore terminal input")?;
            self.active = false;
        }
        Ok(())
    }

    fn resume(&mut self) -> anyhow::Result<()> {
        if !self.active {
            terminal::enable_raw_mode().context("cannot enable terminal raw mode")?;
            execute!(std::io::stdout(), EnterAlternateScreen, Hide)
                .context("cannot enter the alternate screen")?;
            self.active = true;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
}

/// The frame currently on the terminal, so an identical one is not written again.
///
/// The loop wakes on a 250 ms tick whether or not anything moved, and a frame
/// begins by erasing the display. Repainting that unchanged frame four times a
/// second is what stops a terminal that anchors a selection to the text under it —
/// VS Code's — from letting anyone select a URL off this screen: the selection is
/// dropped by the next tick's erase. So the frame is built into a buffer and
/// compared, and an idle control plane writes nothing at all.
#[derive(Default)]
struct Screen {
    shown: Vec<u8>,
}

impl Screen {
    fn draw(&mut self, frame: Vec<u8>, out: &mut impl Write) -> anyhow::Result<()> {
        if frame == self.shown {
            return Ok(());
        }
        out.write_all(&frame).context("cannot write to the terminal")?;
        out.flush().context("cannot write to the terminal")?;
        self.shown = frame;
        Ok(())
    }

    /// Forget what is on screen, for when something else has been writing to it.
    fn invalidate(&mut self) {
        self.shown.clear();
    }
}

/// What the screen is showing. One value rather than a flag per overlay, because
/// the list, the name prompt and a page each take the keyboard whole.
enum View {
    List,
    /// A new instance's name, as it is being typed.
    Naming(String),
    /// A page to read — one instance's specs, or the keys — as built when it was
    /// opened, scrolled by `offset` lines.
    Page {
        title: String,
        lines: Vec<String>,
        offset: usize,
    },
}

/// What a key does on the instance list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Up,
    Down,
    Specs,
    Start,
    Stop,
    Restart,
    Open,
    Edit,
    Password,
    New,
    StartAll,
    Rescan,
    NewPin,
    CopyPin,
    Keys,
    Quit,
}

/// Which heading of the keys page a binding is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Selected,
    Instances,
    ControlPlane,
}

impl Group {
    const ALL: [Self; 3] = [Self::Selected, Self::Instances, Self::ControlPlane];

    fn heading(self) -> &'static str {
        match self {
            Self::Selected => "the selected instance",
            Self::Instances => "instances",
            Self::ControlPlane => "control plane",
        }
    }
}

/// One action's keys and the words the screen says about it.
struct Binding {
    action: Action,
    codes: &'static [KeyCode],
    /// The word the key bar shows after the key.
    label: &'static str,
    /// The keys page's sentence.
    about: &'static str,
    group: Group,
    /// Whether the key bar under the list shows it. Every binding is on the keys
    /// page; the bar keeps to what fits one line of an 80-column terminal.
    bar: bool,
}

/// Every key the list takes. The key bar and the keys page are drawn from this
/// and the key loop looks its actions up here, so an action added here is one
/// the screen already explains.
const BINDINGS: &[Binding] = &[
    Binding {
        action: Action::Up,
        codes: &[KeyCode::Up, KeyCode::Char('k')],
        label: "up",
        about: "select the instance above",
        group: Group::Instances,
        bar: false,
    },
    Binding {
        action: Action::Down,
        codes: &[KeyCode::Down, KeyCode::Char('j')],
        label: "down",
        about: "select the instance below",
        group: Group::Instances,
        bar: false,
    },
    Binding {
        action: Action::Specs,
        codes: &[KeyCode::Enter],
        label: "specs",
        about: "show its settings and what each target will do",
        group: Group::Selected,
        bar: true,
    },
    Binding {
        action: Action::Start,
        codes: &[KeyCode::Char('s')],
        label: "start",
        about: "start it",
        group: Group::Selected,
        bar: true,
    },
    Binding {
        action: Action::Stop,
        codes: &[KeyCode::Char('x')],
        label: "stop",
        about: "stop it",
        group: Group::Selected,
        bar: true,
    },
    Binding {
        action: Action::Restart,
        codes: &[KeyCode::Char('r')],
        label: "restart",
        about: "restart it, which applies an edited config",
        group: Group::Selected,
        bar: false,
    },
    Binding {
        action: Action::Open,
        codes: &[KeyCode::Char('o')],
        label: "open",
        about: "open it in the browser, once it is running",
        group: Group::Selected,
        bar: true,
    },
    Binding {
        action: Action::Edit,
        codes: &[KeyCode::Char('e')],
        label: "edit",
        about: "edit its zdesk.toml in $VISUAL or $EDITOR",
        group: Group::Selected,
        bar: true,
    },
    Binding {
        action: Action::Password,
        codes: &[KeyCode::Char('p')],
        label: "password",
        about: "encrypt a password into its [passwords]",
        group: Group::Selected,
        bar: false,
    },
    Binding {
        action: Action::New,
        codes: &[KeyCode::Char('n')],
        label: "new",
        about: "create an instance",
        group: Group::Instances,
        bar: true,
    },
    Binding {
        action: Action::StartAll,
        codes: &[KeyCode::Char('a')],
        label: "start all",
        about: "start every stopped instance",
        group: Group::Instances,
        bar: false,
    },
    Binding {
        action: Action::Rescan,
        codes: &[KeyCode::Char('R')],
        label: "rescan",
        about: "rescan the instances directory",
        group: Group::Instances,
        bar: false,
    },
    Binding {
        action: Action::NewPin,
        codes: &[KeyCode::Char('P')],
        label: "new PIN",
        about: "a new login PIN, its time started again; logged-in browsers stay in",
        group: Group::ControlPlane,
        bar: false,
    },
    Binding {
        action: Action::CopyPin,
        codes: &[KeyCode::Char('c')],
        label: "copy PIN",
        about: "send the PIN to the clipboard through the terminal (OSC 52, or a tmux buffer); Apple's Terminal.app drops it",
        group: Group::ControlPlane,
        bar: false,
    },
    Binding {
        action: Action::Keys,
        codes: &[KeyCode::Char('?')],
        label: "keys",
        about: "show every key",
        group: Group::ControlPlane,
        bar: true,
    },
    Binding {
        action: Action::Quit,
        codes: &[KeyCode::Char('q'), KeyCode::Esc],
        label: "quit",
        about: "stop every instance and quit, as Ctrl+C does",
        group: Group::ControlPlane,
        bar: true,
    },
];

fn action_for(code: KeyCode) -> Option<Action> {
    BINDINGS
        .iter()
        .find(|binding| binding.codes.contains(&code))
        .map(|binding| binding.action)
}

fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Enter => "Enter".to_owned(),
        KeyCode::Esc => "Esc".to_owned(),
        KeyCode::Char(character) => character.to_string(),
        other => format!("{other:?}"),
    }
}

/// The keys page: every binding under its heading.
fn key_lines() -> Vec<String> {
    let mut lines = Vec::new();
    for group in Group::ALL {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(group.heading().to_owned());
        for binding in BINDINGS.iter().filter(|binding| binding.group == group) {
            let keys: Vec<_> = binding.codes.iter().map(|code| key_name(*code)).collect();
            lines.push(format!("  {:<10} {}", keys.join(" "), binding.about));
        }
    }
    lines
}

/// The key bar under the list, in the order the bindings are listed, as much of
/// it as fits `width`. What does not fit goes from the end of the instance keys
/// first, so the keys page and quitting stay named on a narrow terminal, and then
/// from the front of the rest, so quitting is the last key named.
fn list_bar(width: u16) -> Vec<(String, &'static str)> {
    let mut shown: Vec<_> = BINDINGS.iter().filter(|binding| binding.bar).collect();
    while bar_width(&shown) > width as usize && !shown.is_empty() {
        let drop = shown
            .iter()
            .rposition(|binding| binding.group != Group::ControlPlane)
            .unwrap_or(0);
        shown.remove(drop);
    }
    shown
        .iter()
        .map(|binding| (key_name(binding.codes[0]), binding.label))
        .collect()
}

/// The columns a key bar of these bindings takes: each key, a space and its
/// word, two spaces between pairs.
fn bar_width(bindings: &[&Binding]) -> usize {
    let pairs: usize = bindings
        .iter()
        .map(|binding| key_name(binding.codes[0]).chars().count() + 1 + binding.label.chars().count())
        .sum();
    pairs + 2 * bindings.len().saturating_sub(1)
}

/// A run of text in its colour and weight, as a row of the screen is drawn.
type Run = (String, Option<Color>, bool);

/// The bytes that put `text` on the clipboard through the terminal: OSC 52,
/// which the terminal emulator answers, so it reaches the clipboard of the
/// machine whose screen this is, through SSH. iTerm2, kitty, WezTerm, Ghostty,
/// Alacritty, foot and Windows Terminal take it; Apple's Terminal.app drops it,
/// which is what the plain PIN on screen is for.
fn clipboard_write(text: &str) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07").into_bytes()
}

/// Put `text` on the clipboard. Inside tmux, through a tmux buffer: tmux's
/// default `set-clipboard external` drops an application's OSC 52 but sends
/// tmux's own copies on to the outer terminal, which `set-buffer -w` asks for.
/// Elsewhere, OSC 52 straight to the terminal.
async fn copy_pin(text: &str) -> anyhow::Result<()> {
    if std::env::var_os("TMUX").is_some_and(|value| !value.is_empty()) {
        let output = Command::new("tmux")
            .args(["set-buffer", "-w", "--", text])
            .stdin(Stdio::null())
            .output()
            .await
            .context("cannot run tmux")?;
        anyhow::ensure!(
            output.status.success(),
            "tmux set-buffer -w failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(());
    }
    let mut out = std::io::stdout().lock();
    out.write_all(&clipboard_write(text))
        .and_then(|()| out.flush())
        .context("cannot write to the terminal")
}

/// What the status line says once the clipboard write has gone out. No terminal
/// answers it, and the environment here says nothing sure about the terminal at
/// the other end of an SSH session, so the write is reported as sent, not done.
const UNCONFIRMED_COPY: &str = "PIN sent to the clipboard; if a paste does not give it, select the PIN instead";

/// A six-digit PIN as it is read out and typed: `123-456`. The dash keeps it one
/// word for a double-click, and the PIN field drops it.
fn dashed(digits: &str) -> String {
    match digits.split_at_checked(3) {
        Some((first, second)) => format!("{first}-{second}"),
        None => digits.to_owned(),
    }
}

/// The access panel's three rows: the PIN a browser logs in with, in a colour
/// of its own beside its label, or why no new browser can log in with one.
fn access_rows(pin: &PinState) -> [Vec<Run>; 3] {
    let hint = ("press P for a new PIN · c copies it".to_owned(), Some(Color::DarkGrey), false);
    // Browsers already logged in stay in whatever the PIN's state; only a new
    // login needs it.
    let refused = "no new browser can log in · logged-in ones stay in";
    let (heading, detail) = match pin {
        PinState::Open {
            digits,
            expires_at,
            minutes_left,
        } => (
            vec![
                ("PIN ".to_owned(), Some(Color::Green), true),
                (dashed(digits), Some(Color::Magenta), true),
            ],
            format!("expires {expires_at} · {minutes_left} min left"),
        ),
        PinState::Expired => (
            vec![("PIN expired".to_owned(), Some(Color::Yellow), true)],
            refused.to_owned(),
        ),
        PinState::Withdrawn => (
            vec![(format!("PIN withdrawn after {MAX_WRONG} wrong entries"), Some(Color::Red), true)],
            refused.to_owned(),
        ),
    };
    [heading, vec![(detail, None, false)], vec![hint]]
}

/// The rows above the list: the title, the instances directory, a blank row,
/// the access panel's five, a blank row and the column headings.
const LIST_TOP: u16 = 10;
/// The rows below it: a blank row, the rule, the message and the key bar.
const LIST_BOTTOM: u16 = 4;

fn render(
    size: (u16, u16),
    options: &TuiOptions,
    pin: &PinState,
    instances: &[InstanceInfo],
    selected: usize,
    view: &View,
    message: &str,
) -> anyhow::Result<Vec<u8>> {
    let (width, height) = size;
    let port = options.port;
    let mut frame = Vec::new();
    queue!(frame, MoveTo(0, 0), Clear(ClearType::All))?;
    if let View::Page { title, lines, offset } = view {
        render_page(&mut frame, width, height, title, lines, *offset)?;
        return Ok(frame);
    }
    line(
        &mut frame,
        0,
        width,
        &format!("zDesk {VERSION} · http://{MASTER_HOST}:{port}"),
        Some(Color::Cyan),
        true,
    )?;
    line(
        &mut frame,
        1,
        width,
        &format!("instances {}", options.instances_dir.display()),
        Some(Color::DarkGrey),
        false,
    )?;
    let access = access_rows(pin);
    panel(&mut frame, 3, width, "access PIN", &access)?;
    line(
        &mut frame,
        LIST_TOP - 1,
        width,
        "  instance             state      URL",
        Some(Color::DarkGrey),
        false,
    )?;

    let available = height.saturating_sub(LIST_TOP + LIST_BOTTOM) as usize;
    let start = if selected >= available && available > 0 {
        selected + 1 - available
    } else {
        0
    };
    for (row, (index, instance)) in instances.iter().enumerate().skip(start).take(available).enumerate() {
        let marker = if index == selected { '›' } else { ' ' };
        let text = format!(
            "{marker} {:<20} {:<10} {}",
            instance.name,
            instance.status.label(),
            instance_url(&instance.name, port)
        );
        line(
            &mut frame,
            LIST_TOP + row as u16,
            width,
            &text,
            (index == selected).then_some(Color::Yellow),
            index == selected,
        )?;
    }
    if instances.is_empty() {
        line(&mut frame, LIST_TOP, width, "  no instances — press n to create one", Some(Color::Yellow), false)?;
    }

    let rule = height.saturating_sub(3);
    line(&mut frame, rule, width, &"─".repeat(width as usize), Some(Color::DarkGrey), false)?;
    if let View::Naming(name) = view {
        line(
            &mut frame,
            rule + 1,
            width,
            &format!("new instance name: {name}_"),
            Some(Color::Yellow),
            true,
        )?;
        key_bar(&mut frame, rule + 2, width, &[("Enter".to_owned(), "create"), ("Esc".to_owned(), "cancel")])?;
    } else {
        let detail = instances
            .get(selected)
            .and_then(|instance| instance.detail.as_deref())
            .unwrap_or(message);
        line(&mut frame, rule + 1, width, detail, None, false)?;
        key_bar(&mut frame, rule + 2, width, &list_bar(width))?;
    }
    Ok(frame)
}

fn render_page(
    frame: &mut Vec<u8>,
    width: u16,
    height: u16,
    title: &str,
    lines: &[String],
    offset: usize,
) -> anyhow::Result<()> {
    line(frame, 0, width, title, Some(Color::Cyan), true)?;
    let available = height.saturating_sub(4) as usize;
    for (row, text) in lines.iter().skip(offset).take(available).enumerate() {
        line(frame, 2 + row as u16, width, text, None, false)?;
    }
    let mut keys = vec![("↑↓".to_owned(), "scroll"), ("Esc".to_owned(), "back")];
    if offset + available < lines.len() {
        keys.push(("…".to_owned(), "more below"));
    }
    key_bar(frame, height.saturating_sub(1), width, &keys)?;
    Ok(())
}

/// A boxed one-line panel across the screen, three rows from `top`, with its
/// title in the top border. Too narrow for a box, it is the text alone.
/// The columns inside a panel's borders on a screen `width` wide.
fn panel_inner(width: u16) -> usize {
    (width as usize).saturating_sub(4)
}

/// A box from row `top`, its title in the top border and `rows` inside, each
/// cut to fit. Too narrow for the border, the rows alone.
fn panel(frame: &mut Vec<u8>, top: u16, width: u16, title: &str, rows: &[Vec<Run>]) -> std::io::Result<()> {
    let inner = panel_inner(width);
    let border = 4 + title.chars().count();
    let bordered = inner > 0 && border < width as usize;
    if bordered {
        let top_border = format!("┌ {title} {}┐", "─".repeat(width as usize - border));
        line(frame, top, width, &top_border, Some(Color::DarkGrey), false)?;
    }
    for (index, row) in rows.iter().enumerate() {
        let at = top + 1 + index as u16;
        let runs: Vec<_> = row.iter().map(|(text, color, bold)| (text.as_str(), *color, *bold)).collect();
        if !bordered {
            spans(frame, at, width, &runs)?;
            continue;
        }
        let mut left = inner;
        let mut cut = Vec::new();
        for (text, color, bold) in runs {
            let clipped: String = text.chars().take(left).collect();
            left -= clipped.chars().count();
            cut.push((clipped, color, bold));
        }
        let padding = " ".repeat(left);
        let mut boxed = vec![("│ ", Some(Color::DarkGrey), false)];
        boxed.extend(cut.iter().map(|(text, color, bold)| (text.as_str(), *color, *bold)));
        boxed.push((&padding, None, false));
        boxed.push((" │", Some(Color::DarkGrey), false));
        spans(frame, at, width, &boxed)?;
    }
    if bordered {
        let bottom = format!("└{}┘", "─".repeat(width as usize - 2));
        line(frame, top + 1 + rows.len() as u16, width, &bottom, Some(Color::DarkGrey), false)?;
    }
    Ok(())
}

/// Keys and what they do, the key bold and its word dim. A pair that would not
/// fit whole is left off rather than cut through.
fn key_bar(frame: &mut Vec<u8>, row: u16, width: u16, keys: &[(String, &str)]) -> std::io::Result<()> {
    let mut parts = Vec::new();
    let mut used = 0;
    for (key, label) in keys {
        let gap = if parts.is_empty() { "" } else { "  " };
        let length = gap.chars().count() + key.chars().count() + 1 + label.chars().count();
        if used + length > width as usize {
            break;
        }
        used += length;
        parts.push((gap, key.as_str(), *label));
    }
    let words: Vec<_> = parts.iter().map(|(_, _, label)| format!(" {label}")).collect();
    let mut runs = Vec::new();
    for ((gap, key, _), word) in parts.iter().zip(&words) {
        runs.push((*gap, None, false));
        runs.push((*key, None, true));
        runs.push((word.as_str(), Some(Color::DarkGrey), false));
    }
    spans(frame, row, width, &runs)
}

/// Everything known about one instance, as the specs page's lines: its own state
/// and paths, then what its config says each target will do.
///
/// Read from the file when the page is opened rather than from the child process,
/// because there is nothing to ask a child — the gateway parses this config at
/// launch and keeps no channel back. So for a running instance this is the config
/// it *would* start on, which is exactly what somebody who has just edited it
/// wants to check, and the same reason `e` says to restart.
fn describe_instance(instance: &InstanceInfo, port: u16) -> Vec<String> {
    let mut lines = vec![
        spec("state", instance.status.label()),
        spec("url", &instance_url(&instance.name, port)),
        spec("config", &instance.config_path().display().to_string()),
        spec("log", &instance.log_path().display().to_string()),
    ];
    if let Some(detail) = &instance.detail {
        lines.push(spec("detail", detail));
    }

    let file = match super::Instance::new(&instance.dir).load() {
        Ok(file) => file,
        Err(error) => {
            lines.push(String::new());
            lines.push(format!("this config will not start: {error:#}"));
            return lines;
        }
    };
    let branding = file
        .branding
        .as_ref()
        .and_then(|branding| branding.text.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(DEFAULT_BRANDING);
    lines.push(spec("shown as", branding));
    lines.push(spec("targets", &file.targets.len().to_string()));

    if file.targets.is_empty() {
        lines.push(String::new());
        lines.push("no targets yet — press e to add one".to_owned());
    }
    for target in &file.targets {
        lines.push(String::new());
        lines.push(format!("target {}", target.name));
        lines.extend(target_specs(target));
    }
    lines
}

/// One `[[targets]]` profile as sentences: what this target will do, not which
/// keys were written. Credentials are reported as present, never printed — this
/// page is on somebody's screen, and the passwords are the reason the instance
/// directory is `0700`.
fn target_specs(target: &TargetConfig) -> Vec<String> {
    let mut lines = Vec::new();
    let protocol = match target.subtype {
        None => target.protocol.name().to_owned(),
        Some(subtype) => format!("{} {}", target.protocol.name(), subtype.name()),
    };
    lines.push(spec("protocol", &protocol));
    lines.push(spec("address", &format!("{}:{}", target.host, target.port)));

    let mut credentials = Vec::new();
    if !target.username.is_empty() {
        credentials.push(format!("user {}", target.username));
    }
    if let Some(domain) = target.domain.as_deref().filter(|domain| !domain.is_empty()) {
        credentials.push(format!("domain {domain}"));
    }
    if let Some(name) = &target.password_ref {
        credentials.push(format!("account password [passwords].{name}"));
    }
    if let Some(name) = &target.vnc_password_ref {
        credentials.push(format!("vnc password [passwords].{name}"));
    }
    if credentials.is_empty() {
        credentials.push("none configured".to_owned());
    }
    lines.push(spec("sign-in", &credentials.join(", ")));

    lines.push(spec("size", &describe_size(target)));
    lines.push(spec("picker", &describe_offers(target)));

    if target.protocol == Protocol::Rdp {
        lines.push(spec("security", "nla — the credentials are checked before the session"));
        lines.push(spec(
            "graphics",
            if target.egfx() {
                "egfx pipeline; a resize is a graphics reset"
            } else {
                "bitmap updates at the opening size"
            },
        ));
    }

    // Beside the clipboard rather than inside the block above: sound is the
    // protocol's own question, and it is VNC that answers it today.
    lines.push(spec("audio", &describe_audio(target)));
    lines.push(spec("render", &target.render_summary()));
    lines
}

/// The size a session on this target has: the one it keeps, and the window where
/// the picker offers following it.
fn describe_size(target: &TargetConfig) -> String {
    if !target.sized() {
        return "the remote's own".to_owned();
    }
    let (w, h) = DEFAULT_SIZE;
    match (target.size, target.offers().resize) {
        (Some((w, h)), true) => format!("{w}×{h} points, or the client's window"),
        (Some((w, h)), false) => format!("{w}×{h} points"),
        (None, true) => format!("the client's window, or {w}×{h} points on a phone"),
        (None, false) => format!("{w}×{h} points, the default"),
    }
}

/// What whoever starts a session on this target chooses at the picker, which is
/// its type's to offer and not a key of the file.
fn describe_offers(target: &TargetConfig) -> String {
    let offers = target.offers();
    let mut choices = Vec::new();
    if offers.resize {
        choices.push("resize".to_owned());
    }
    if offers.audio {
        choices.push("sound".to_owned());
    }
    if offers.camera {
        choices.push("camera".to_owned());
    }
    if offers.microphone {
        choices.push("microphone".to_owned());
    }
    // One passed in every session is no choice, and the render line names it;
    // one passed where no other is named, wlshare's VP9, is part of the choice.
    let chosen = offers.passthrough.len() > 1;
    for passthrough in offers.passthrough.iter().filter(|passthrough| chosen || passthrough.chosen()) {
        choices.push(format!("{} passed through", passthrough.stream()));
    }
    if offers.placement {
        choices.push("where the second display sits".to_owned());
    }
    if offers.pipeline {
        choices.push("high quality or adaptive H.264".to_owned());
    }
    if choices.is_empty() {
        "nothing to choose".to_owned()
    } else {
        format!("offers {}", choices.join(", "))
    }
}

/// A target's audio as it will sound on the wire, in a session that takes it as
/// Opus. Kilobits because a person reads them, and the codec named the way
/// `ServerMsg::AudioFormat` names it. Lossless is the picker's other format and
/// has no key to describe.
fn describe_audio(target: &TargetConfig) -> String {
    if target.media_stream() {
        return "the Mac's AAC-ELD, passed through".to_owned();
    }
    if !target.offers().audio {
        return "none".to_owned();
    }
    let ceiling = crate::config::AUDIO_BITRATE_KBPS;
    format!("opus ≤{ceiling} kbit/s, adaptive down to {} kbit/s, or flac", sound_opus::walk::BITRATE_FLOOR / 1000)
}

/// One `label   value` row of a specs page, indented under its heading.
fn spec(label: &str, value: &str) -> String {
    format!("  {label:<10} {value}")
}

fn line(
    stdout: &mut impl Write,
    row: u16,
    width: u16,
    text: &str,
    color: Option<Color>,
    bold: bool,
) -> std::io::Result<()> {
    spans(stdout, row, width, &[(text, color, bold)])
}

/// One row of runs, each in its own colour and weight, clipped to `width` as a
/// whole.
fn spans(
    stdout: &mut impl Write,
    row: u16,
    width: u16,
    runs: &[(&str, Option<Color>, bool)],
) -> std::io::Result<()> {
    queue!(stdout, MoveTo(0, row))?;
    let mut left = width as usize;
    for (text, color, bold) in runs {
        let clipped: String = text.chars().take(left).collect();
        left -= clipped.chars().count();
        if let Some(color) = color {
            queue!(stdout, SetForegroundColor(*color))?;
        }
        if *bold {
            queue!(stdout, SetAttribute(Attribute::Bold))?;
        }
        queue!(stdout, Print(clipped), SetAttribute(Attribute::Reset), ResetColor)?;
    }
    Ok(())
}

/// Stable state shown by both the TUI and the master landing page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceStatus {
    Stopped,
    Starting,
    Running,
    /// The gateway ended cleanly without this manager asking: exit code 0, or a
    /// SIGINT/SIGTERM/SIGHUP from outside. Nothing about it failed.
    Exited,
    /// The gateway could not start, exited with a non-zero code, or died on a
    /// crash signal.
    Failed,
}

impl InstanceStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
        }
    }
}

/// One row of the manager's public state, with no launch token or endpoint.
#[derive(Clone, Debug)]
pub struct InstanceInfo {
    pub name: String,
    pub status: InstanceStatus,
    /// The instance directory, which is where every path this instance has comes
    /// from — see [`super::Instance`] for the ones the gateway itself uses.
    pub dir: PathBuf,
    pub detail: Option<String>,
}

impl InstanceInfo {
    /// The one file a user edits.
    pub fn config_path(&self) -> PathBuf {
        super::Instance::new(&self.dir).config_path()
    }

    /// Where a gateway that ended said why.
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("gateway.log")
    }
}

enum InstanceState {
    Stopped,
    Starting,
    /// Boxed because a Windows child process carries its handles and job state
    /// inline, which makes this variant many times the size of the others.
    Running(Box<RunningGateway>),
    /// A clean exit this manager did not request; the string says how it ended.
    Exited(String),
    Failed(String),
}

struct ManagedInstance {
    name: String,
    dir: PathBuf,
    state: InstanceState,
}

struct RunningGateway {
    child: Child,
    endpoint: String,
    token: String,
}

/// Owns every instance subprocess. Dropping the process closes every child's
/// liveness pipe; [`Self::shutdown`] is the polite path on top of that guarantee.
pub struct Supervisor {
    root: PathBuf,
    binary: PathBuf,
    instances: Vec<ManagedInstance>,
    routes: RouteTable,
}

impl Supervisor {
    pub async fn open(root: PathBuf, binary: PathBuf) -> anyhow::Result<Self> {
        create_private_dir(&root)?;
        let mut manager = Self {
            root,
            binary,
            instances: Vec::new(),
            routes: RouteTable::default(),
        };
        manager.rescan().await?;
        Ok(manager)
    }

    pub fn routes(&self) -> RouteTable {
        self.routes.clone()
    }

    pub fn instances(&self) -> Vec<InstanceInfo> {
        self.instances
            .iter()
            .map(|instance| {
                let (status, detail) = match &instance.state {
                    InstanceState::Stopped => (InstanceStatus::Stopped, None),
                    InstanceState::Starting => (InstanceStatus::Starting, None),
                    InstanceState::Running(_) => (InstanceStatus::Running, None),
                    InstanceState::Exited(how) => (InstanceStatus::Exited, Some(how.clone())),
                    InstanceState::Failed(error) => (InstanceStatus::Failed, Some(error.clone())),
                };
                InstanceInfo {
                    name: instance.name.clone(),
                    status,
                    dir: instance.dir.clone(),
                    detail,
                }
            })
            .collect()
    }

    pub async fn rescan(&mut self) -> anyhow::Result<()> {
        let mut names = BTreeSet::new();
        for entry in std::fs::read_dir(&self.root)
            .with_context(|| format!("cannot list {}", self.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_instance_name(&name).is_ok() {
                names.insert(name);
            }
        }

        let mut previous: BTreeMap<_, _> = std::mem::take(&mut self.instances)
            .into_iter()
            .map(|instance| (instance.name.clone(), instance))
            .collect();
        for name in names {
            if let Some(instance) = previous.remove(&name) {
                self.instances.push(instance);
            } else {
                let dir = self.root.join(&name);
                // A directory made or copied in by hand keeps whatever access it came
                // with until it is made private here, the files already in it included.
                transport::make_private(&dir)?;
                bootstrap_config(&dir)?;
                self.instances.push(ManagedInstance {
                    name,
                    dir,
                    state: InstanceState::Stopped,
                });
            }
        }
        // A running process stays manageable even if its directory was renamed or
        // removed behind the TUI. Stopped vanished entries simply leave the list.
        self.instances.extend(previous.into_values().filter(|instance| {
            matches!(instance.state, InstanceState::Running(_) | InstanceState::Starting)
        }));
        self.instances.sort_by(|a, b| a.name.cmp(&b.name));
        self.publish().await;
        Ok(())
    }

    pub async fn create(&mut self, name: &str) -> anyhow::Result<usize> {
        valid_instance_name(name)?;
        let dir = self.root.join(name);
        std::fs::create_dir(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        transport::make_private(&dir)?;
        if let Err(error) = bootstrap_config(&dir) {
            let _ = std::fs::remove_dir(&dir);
            return Err(error);
        }
        self.rescan().await?;
        self.instances
            .iter()
            .position(|instance| instance.name == name)
            .context("the new instance did not appear after rescan")
    }

    /// Start an instance, handing its worker `key` — the one that opens its
    /// config's `[passwords]`, or `None` for a config that names none.
    pub async fn start(&mut self, name: &str, key: Option<&Key>) -> anyhow::Result<()> {
        let index = self.index(name)?;
        if matches!(
            self.instances[index].state,
            InstanceState::Running(_) | InstanceState::Starting
        ) {
            return Ok(());
        }
        let dir = self.instances[index].dir.clone();
        let instance = super::Instance::new(&dir);
        validate_config(&instance.config_path())
            .with_context(|| format!("instance {name:?} has an invalid config"))?;
        // Asked here as well as by the worker, which holds the claim, so an
        // instance another control plane is serving says so instead of ending in a
        // handshake that never comes. A start that races past this one still meets
        // the worker's.
        drop(instance.claim().await?);
        self.instances[index].state = InstanceState::Starting;
        self.publish().await;

        match spawn_gateway(&self.binary, &dir, key).await {
            Ok(gateway) => {
                self.instances[index].state = InstanceState::Running(Box::new(gateway));
                self.publish().await;
                Ok(())
            }
            Err(error) => {
                // The state carries a rendering because the TUI prints it; the
                // caller gets the error itself, cause chain intact.
                self.instances[index].state = InstanceState::Failed(format!("{error:#}"));
                self.publish().await;
                Err(error)
            }
        }
    }

    pub async fn stop(&mut self, name: &str) -> anyhow::Result<()> {
        let index = self.index(name)?;
        let state = std::mem::replace(&mut self.instances[index].state, InstanceState::Stopped);
        if let InstanceState::Running(gateway) = state {
            stop_gateway(*gateway).await;
        }
        self.publish().await;
        Ok(())
    }

    pub async fn restart(&mut self, name: &str, key: Option<&Key>) -> anyhow::Result<()> {
        self.stop(name).await?;
        self.start(name, key).await
    }

    /// Notice gateways that ended without being asked. Returns a one-line
    /// account of the last one for the status line, or `None` when nothing
    /// changed.
    pub async fn poll_exits(&mut self) -> anyhow::Result<Option<String>> {
        let mut ended = None;
        for instance in &mut self.instances {
            let InstanceState::Running(gateway) = &mut instance.state else {
                continue;
            };
            if let Some(status) = gateway.child.try_wait().context("cannot inspect gateway child")? {
                let exit = ExitKind::of(status);
                let detail = format!("{exit}; see {}", instance.dir.join("gateway.log").display());
                ended = Some(format!("{}: {exit}", instance.name));
                instance.state = if exit.is_clean() {
                    InstanceState::Exited(detail)
                } else {
                    InstanceState::Failed(detail)
                };
            }
        }
        if ended.is_some() {
            self.publish().await;
        }
        Ok(ended)
    }

    pub async fn shutdown(&mut self) {
        for instance in &mut self.instances {
            let state = std::mem::replace(&mut instance.state, InstanceState::Stopped);
            if let InstanceState::Running(gateway) = state {
                stop_gateway(*gateway).await;
            }
        }
        self.publish().await;
    }

    fn index(&self, name: &str) -> anyhow::Result<usize> {
        self.instances
            .iter()
            .position(|instance| instance.name == name)
            .with_context(|| format!("unknown instance {name:?}"))
    }

    async fn publish(&self) {
        let mut published = BTreeMap::new();
        for instance in &self.instances {
            let (status, target) = match &instance.state {
                InstanceState::Stopped => (InstanceStatus::Stopped, None),
                InstanceState::Starting => (InstanceStatus::Starting, None),
                InstanceState::Exited(_) => (InstanceStatus::Exited, None),
                InstanceState::Failed(_) => (InstanceStatus::Failed, None),
                InstanceState::Running(gateway) => (
                    InstanceStatus::Running,
                    Some(RouteTarget {
                        endpoint: gateway.endpoint.clone(),
                        token: gateway.token.clone(),
                    }),
                ),
            };
            published.insert(instance.name.clone(), PublishedInstance { status, target });
        }
        *self.routes.inner.write().await = published;
    }
}

/// How a gateway this manager did not stop came to end.
///
/// The gateway exits 0 on SIGINT or SIGTERM after logging "shutdown signal
/// received", and on its stdin closing; a `pkill` aimed at some other zdesk
/// therefore shows up here as a clean exit, not a crash. Only a non-zero code
/// or a crash is a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitKind {
    /// Exit code 0.
    Clean,
    /// Exit code other than 0.
    Code(i32),
    /// Ended from outside on purpose: SIGINT, SIGTERM or SIGHUP on Unix, and on
    /// Windows the status of a process a Ctrl+C or Ctrl+Break ended.
    Stopped(Termination),
    /// Anything else that ended it: SIGSEGV, SIGABRT, SIGKILL and so on, or on
    /// Windows an exception status such as an access violation or a fail-fast.
    Crashed(Termination),
}

/// What ended a gateway that did not exit by itself: a signal number on Unix, an
/// `NTSTATUS` on Windows.
#[cfg(unix)]
type Termination = i32;
#[cfg(windows)]
type Termination = u32;

impl ExitKind {
    #[cfg(unix)]
    fn of(status: std::process::ExitStatus) -> Self {
        use std::os::unix::process::ExitStatusExt as _;
        match (status.code(), status.signal()) {
            (Some(0), _) => Self::Clean,
            (Some(code), _) => Self::Code(code),
            (None, Some(signal)) if [libc::SIGINT, libc::SIGTERM, libc::SIGHUP].contains(&signal) => {
                Self::Stopped(signal)
            }
            (None, Some(signal)) => Self::Crashed(signal),
            (None, None) => Self::Crashed(0),
        }
    }

    /// Windows has one number for all of it. An exit code with both severity bits
    /// set is an `NTSTATUS` error, which a process does not choose to exit with:
    /// the system ended it. A `TerminateProcess` from outside is not among them —
    /// it exits with whatever code its caller named — and reads as that code.
    #[cfg(windows)]
    fn of(status: std::process::ExitStatus) -> Self {
        match status.code().map(|code| code as u32) {
            Some(0) => Self::Clean,
            Some(status @ STATUS_CONTROL_C_EXIT) => Self::Stopped(status),
            Some(status) if status & 0xC000_0000 == 0xC000_0000 => Self::Crashed(status),
            Some(code) => Self::Code(code as i32),
            None => Self::Crashed(0),
        }
    }

    fn is_clean(self) -> bool {
        matches!(self, Self::Clean | Self::Stopped(_))
    }
}

#[cfg(unix)]
fn termination_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        _ => "an uncommon signal",
    }
}

#[cfg(windows)]
const STATUS_CONTROL_C_EXIT: u32 = 0xC000_013A;

#[cfg(windows)]
fn termination_name(status: u32) -> &'static str {
    match status {
        0xC000_0005 => "STATUS_ACCESS_VIOLATION",
        0xC000_0017 => "STATUS_NO_MEMORY",
        0xC000_001D => "STATUS_ILLEGAL_INSTRUCTION",
        0xC000_0094 => "STATUS_INTEGER_DIVIDE_BY_ZERO",
        0xC000_00FD => "STATUS_STACK_OVERFLOW",
        STATUS_CONTROL_C_EXIT => "STATUS_CONTROL_C_EXIT",
        0xC000_0374 => "STATUS_HEAP_CORRUPTION",
        // What a Rust panic under `panic = "abort"` ends in: `abort` is a fail-fast.
        0xC000_0409 => "STATUS_STACK_BUFFER_OVERRUN, a fail-fast such as an abort",
        _ => "an uncommon status",
    }
}

impl std::fmt::Display for ExitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            #[cfg(unix)]
            Self::Clean => write!(
                f,
                "the gateway shut down cleanly (exit code 0) without this control plane asking; \
                 something else sent it SIGINT or SIGTERM, such as a pkill matching 'zdesk serve'"
            ),
            // A worker has a console of its own on Windows, so no Ctrl+C typed
            // anywhere reaches it: there is no outside hand to name.
            #[cfg(windows)]
            Self::Clean => write!(
                f,
                "the gateway shut down cleanly (exit code 0) without this control plane asking"
            ),
            Self::Code(code) => write!(f, "the gateway failed with exit code {code}"),
            #[cfg(unix)]
            Self::Stopped(signal) => write!(
                f,
                "the gateway was stopped from outside by signal {signal} ({})",
                termination_name(signal)
            ),
            #[cfg(windows)]
            Self::Stopped(status) => write!(
                f,
                "the gateway was stopped from outside with status {status:#010X} ({})",
                termination_name(status)
            ),
            #[cfg(unix)]
            Self::Crashed(signal) => write!(
                f,
                "the gateway died on signal {signal} ({})",
                termination_name(signal)
            ),
            #[cfg(windows)]
            Self::Crashed(status) => write!(
                f,
                "the gateway died with status {status:#010X} ({})",
                termination_name(status)
            ),
        }
    }
}

async fn spawn_gateway(binary: &Path, dir: &Path, key: Option<&Key>) -> anyhow::Result<RunningGateway> {
    let log_path = dir.join("gateway.log");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("cannot open {}", log_path.display()))?;
    writeln!(log, "\n--- gateway launch ---")?;
    let stderr = log.try_clone()?;
    let mut command = Command::new(binary);
    command
        .arg("serve-worker")
        .arg("--instance-dir")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    // A console of its own, which nobody sees. Sharing this one would hand every
    // worker the Ctrl+C typed into an editor this TUI runs, and the close event of
    // the window — each a stop the TUI did not ask for. Closing the window still
    // stops them, by ending this process and so closing their stdin.
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot start gateway for {}", dir.display()))?;
    // The key line, first and only: see `super::read_key`. The pipe then stays open
    // for as long as this process wants the worker to live.
    let line = format!("{}\n", key.map(Key::to_line).unwrap_or_default());
    let stdin = child.stdin.as_mut().context("gateway stdin was not piped")?;
    stdin.write_all(line.as_bytes()).await.context("cannot hand the gateway its key")?;
    stdin.flush().await.context("cannot hand the gateway its key")?;
    let stdout = child.stdout.take().context("gateway stdout was not piped")?;
    let mut stdout = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    let bytes = tokio::time::timeout(HANDSHAKE_TIMEOUT, stdout.read_line(&mut line))
        .await
        .context("gateway did not print its handshake within 20 seconds")??;
    anyhow::ensure!(bytes != 0, "gateway exited before printing its handshake; see {}", log_path.display());
    let handshake: Handshake = serde_json::from_str(line.trim_end())
        .with_context(|| format!("gateway printed a malformed handshake: {line:?}"))?;
    transport::check_endpoint(dir, &handshake.endpoint)?;
    anyhow::ensure!(!handshake.token.is_empty(), "gateway returned an empty token");
    Ok(RunningGateway {
        child,
        endpoint: handshake.endpoint,
        token: handshake.token,
    })
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

async fn stop_gateway(mut gateway: RunningGateway) {
    drop(gateway.child.stdin.take());
    if tokio::time::timeout(STOP_GRACE, gateway.child.wait()).await.is_err() {
        let _ = gateway.child.start_kill();
        let _ = gateway.child.wait().await;
    }
}

fn valid_instance_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.is_empty(), "the name is empty");
    anyhow::ensure!(name.len() <= 63, "the name is longer than one DNS label");
    anyhow::ensure!(
        name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "use lowercase ASCII letters, digits, and hyphens only"
    );
    anyhow::ensure!(!name.starts_with('-') && !name.ends_with('-'), "the name may not start or end with '-'");
    Ok(())
}

fn create_private_dir(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("cannot create {}", path.display()))?;
    transport::make_private(path)
}

fn bootstrap_config(dir: &Path) -> anyhow::Result<()> {
    let path = dir.join("zdesk.toml");
    if path.exists() {
        return Ok(());
    }
    let temporary = dir.join(format!("zdesk.toml.{}.new", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // On Windows the file takes the directory's owner-only DACL by inheritance.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("cannot create {}", temporary.display()))?;
        file.write_all(INSTANCE_TEMPLATE.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("cannot install {}", path.display()))
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

#[derive(Clone, Default)]
pub struct RouteTable {
    inner: Arc<RwLock<BTreeMap<String, PublishedInstance>>>,
}

#[derive(Clone)]
struct PublishedInstance {
    status: InstanceStatus,
    target: Option<RouteTarget>,
}

#[derive(Clone)]
struct RouteTarget {
    endpoint: String,
    token: String,
}

/// The one loopback port shared by the landing page and every instance origin.
pub struct SharedPort {
    port: u16,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl SharedPort {
    /// Take both loopbacks at `port`, under `serve`'s binding policy.
    ///
    /// The port is the caller's and is never asked of the kernel: `zdesk.localhost`
    /// and every instance subdomain are typed into a browser, and a port the operator
    /// did not choose is one nobody can type. That is why there is no ephemeral path
    /// here even for tests — a test picks a concrete free port the way `serve`'s do,
    /// so the code under test is the code that ships.
    ///
    /// [`crate::server::bind_all`] is the same all-or-nothing it applies to
    /// `localhost:52380`, and for the same reason one level up: the browser picks the
    /// family, so a master left holding `[::1]:port` from an earlier run would keep
    /// answering and keep routing to *its* instance workers, and which one a page
    /// reached would be the resolver's choice.
    pub async fn bind(port: u16, routes: RouteTable, access: Access) -> anyhow::Result<Self> {
        anyhow::ensure!(port != 0, "the control plane needs a port a browser can be told");
        let addrs = [
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ];
        let mut tasks = Vec::new();
        for listener in crate::server::bind_all(&addrs, &format!("{MASTER_HOST}:{port}"))? {
            listener
                .set_nonblocking(true)
                .context("cannot make a control-plane listener non-blocking")?;
            let listener = tokio::net::TcpListener::from_std(listener)
                .context("cannot hand the control-plane listener to the runtime")?;
            let routes = routes.clone();
            let access = access.clone();
            tasks.push(tokio::spawn(async move {
                loop {
                    let Ok((stream, _peer)) = listener.accept().await else {
                        break;
                    };
                    let routes = routes.clone();
                    let access = access.clone();
                    tokio::spawn(async move {
                        let _ = route_connection(stream, routes, access, port).await;
                    });
                }
            }));
        }
        Ok(Self { port, tasks })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for SharedPort {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn route_connection(
    mut client: tokio::net::TcpStream,
    routes: RouteTable,
    access: Access,
    public_port: u16,
) -> anyhow::Result<()> {
    client.set_nodelay(true)?;
    let request = read_request_head(&mut client).await?;
    let Some(request) = request else {
        return Ok(());
    };
    let parsed = parse_request(&request)?;

    // The master answers only the names it gives out, at its own port, as a
    // development server's allowed hosts do: a tunnel or forward that reaches the
    // port under any other host is refused before anything is read.
    let Some(host) = host_at(&parsed.host, public_port) else {
        write_response(&mut client, "403 Forbidden", "text/plain; charset=utf-8", "blocked host\n", &[]).await?;
        return Ok(());
    };

    // A cookie set on a bare loopback name would be sent to every server on the
    // machine reached by that name, whatever its port: this master's login is
    // set only under its own name.
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        let location = format!("http://{MASTER_HOST}:{public_port}{}", parsed.target);
        write_response(&mut client, "307 Temporary Redirect", "text/plain; charset=utf-8", "", &[("Location", location.as_str())]).await?;
        return Ok(());
    }
    let landing = host == MASTER_HOST;
    let instance = if landing {
        None
    } else {
        match host.strip_suffix(&format!(".{MASTER_HOST}")) {
            Some(name) if valid_instance_name(name).is_ok() => Some(name.to_owned()),
            Some(_) => {
                write_response(&mut client, "404 Not Found", "text/plain; charset=utf-8", "unknown zDesk instance\n", &[]).await?;
                return Ok(());
            }
            None => {
                write_response(&mut client, "403 Forbidden", "text/plain; charset=utf-8", "blocked host\n", &[]).await?;
                return Ok(());
            }
        }
    };

    let login = access.login_in(parsed.cookie.as_deref());
    let path = parsed.target.split('?').next().unwrap_or_default();

    // The PIN is entered on whichever origin the browser is at, and its login is
    // set for the landing origin's domain, so one entry reaches them all.
    if path == PIN_PATH {
        return log_in(client, &access, &parsed, &request).await;
    }

    // The master answers the page's questions on the landing origin, and on an
    // instance origin until the browser is in: then they are its worker's.
    if landing || login.is_none() {
        match (parsed.method.as_str(), path) {
            ("GET", "/api/auth/status") => {
                let status = serde_json::json!({
                    "authenticated": login.is_some(),
                    "login": "pin",
                    "problem": pin_problem(&access.pin()),
                });
                return write_json(&mut client, "200 OK", &status, &[]).await;
            }
            ("GET", "/api/config") => {
                let config = serde_json::json!({ "branding": DEFAULT_BRANDING, "logo": false, "throughput": false });
                return write_json(&mut client, "200 OK", &config, &[]).await;
            }
            _ => {}
        }
    }

    if login.is_none() {
        // The refusal states the version, as a worker's does, since it is the
        // same build's: a page takes a refusal without one for a gateway it
        // was not loaded from, and asks to be reloaded rather than logged in.
        if is_api(path) {
            let refusal = serde_json::json!({ "error": "not logged in" });
            let version = [("X-Zdesk-Version", env!("CARGO_PKG_VERSION"))];
            return write_json(&mut client, "401 Unauthorized", &refusal, &version).await;
        }
        // Any origin's page is the PIN form until the browser is in. On an
        // instance origin its reload is then the worker's page, at the same
        // address. A name with no instance is shown the form too, so the form
        // does not say which instances exist.
        return write_bundle(&mut client, &parsed, path).await;
    }

    let Some(name) = instance else {
        if parsed.method == "GET" && path == "/api/instances" {
            let snapshot = routes.inner.read().await.clone();
            let instances: Vec<_> = snapshot
                .iter()
                .map(|(name, instance)| {
                    serde_json::json!({
                        "name": name,
                        "state": instance.status.label(),
                        "open": (instance.status == InstanceStatus::Running).then(|| instance_url(name, public_port)),
                    })
                })
                .collect();
            return write_json(&mut client, "200 OK", &serde_json::Value::from(instances), &[]).await;
        }
        if is_api(path) {
            return write_json(&mut client, "404 Not Found", &serde_json::json!({ "error": "not found" }), &[]).await;
        }
        return write_bundle(&mut client, &parsed, path).await;
    };

    let published = routes.inner.read().await.get(&name).cloned();
    let Some(published) = published else {
        write_response(&mut client, "404 Not Found", "text/plain; charset=utf-8", "unknown zDesk instance\n", &[]).await?;
        return Ok(());
    };
    let Some(target) = published.target else {
        let body = format!(
            "instance {name} is {}; start it from the zDesk TUI\n",
            published.status.label()
        );
        write_response(&mut client, "503 Service Unavailable", "text/plain; charset=utf-8", &body, &[]).await?;
        return Ok(());
    };

    let mut gateway = match transport::connect(&target.endpoint).await {
        Ok(stream) => stream,
        Err(error) => {
            write_response(
                &mut client,
                "502 Bad Gateway",
                "text/plain; charset=utf-8",
                &format!("instance gateway is unavailable: {error}\n"),
                &[],
            )
            .await?;
            return Ok(());
        }
    };
    gateway.write_all(&forwarded(&request, &parsed, &target.token)?).await?;
    tokio::io::copy_bidirectional(&mut client, &mut gateway).await?;
    Ok(())
}

/// Where the page posts the PIN, on any of the master's origins.
const PIN_PATH: &str = "/api/auth/pin";
/// The most a PIN entry's body may be. It holds six digits and a field name.
const MAX_LOGIN_BODY: usize = 1024;

/// Whether a path is the gateway's API or a socket rather than the page.
fn is_api(path: &str) -> bool {
    path.starts_with("/api/") || path == "/ws" || path.starts_with("/ws/")
}

/// The page's PIN entry.
#[derive(serde::Deserialize)]
struct PinEntry {
    pin: String,
}

/// `POST /api/auth/pin` with `{"pin": "123456"}` on any of the master's origins:
/// a login cookie for the landing origin's domain, and so for it and every
/// instance origin, or what was wrong with the entry.
///
/// A post is taken only with its own origin's `Origin`, which a browser always
/// sends on one: otherwise any page on the web could post wrong entries until the
/// PIN is withdrawn. A local process can claim any origin, as it can post at all,
/// and then it can only use up the PIN's tries, not get in.
async fn log_in(
    mut client: tokio::net::TcpStream,
    access: &Access,
    parsed: &ParsedRequest,
    request: &[u8],
) -> anyhow::Result<()> {
    if parsed.method != "POST" {
        return write_json(&mut client, "405 Method Not Allowed", &serde_json::json!({ "error": "post the PIN" }), &[]).await;
    }
    if parsed.origin.as_deref() != Some(format!("http://{}", parsed.host).as_str()) {
        let refusal = serde_json::json!({ "error": "a PIN is taken only from its own origin's page" });
        return write_json(&mut client, "403 Forbidden", &refusal, &[]).await;
    }
    let body = read_body(&mut client, request, parsed.content_length).await?;
    let Ok(entry) = serde_json::from_slice::<PinEntry>(&body) else {
        let refusal = serde_json::json!({ "error": "the body is not {\"pin\": \"…\"}" });
        return write_json(&mut client, "400 Bad Request", &refusal, &[]).await;
    };
    match access.log_in(&entry.pin) {
        Ok(login) => {
            let cookie = access.set_cookie(&login);
            write_json(&mut client, "200 OK", &serde_json::json!({ "ok": true }), &[("Set-Cookie", cookie.as_str())]).await
        }
        Err(refusal) => {
            write_json(&mut client, "401 Unauthorized", &serde_json::json!({ "error": refusal.message() }), &[]).await
        }
    }
}

/// The bundle's file for `path`, as a gateway serves it: the page for any path
/// that is not a file, isolated, with its validator.
async fn write_bundle(stream: &mut tokio::net::TcpStream, parsed: &ParsedRequest, path: &str) -> anyhow::Result<()> {
    if parsed.method != "GET" {
        write_response(stream, "405 Method Not Allowed", "text/plain; charset=utf-8", "", &[]).await?;
        return Ok(());
    }
    // The software HEVC decoder is a worker's, and the page with it never loads
    // before a login; the document here would read as having found it.
    if path.starts_with("/hevc/") {
        write_response(stream, "404 Not Found", "text/plain; charset=utf-8", "", &[]).await?;
        return Ok(());
    }
    let asset = crate::assets::asset(path);
    let etag = asset.etag.to_str().context("an embedded file's validator is ASCII")?;
    let content_type = asset.content_type.to_str().context("an embedded file's type is ASCII")?;
    let mut headers = crate::assets::ISOLATION.to_vec();
    headers.push(("ETag", etag));
    if parsed.if_none_match.as_deref() == Some(etag) {
        write_response(stream, "304 Not Modified", content_type, "", &headers).await?;
    } else {
        write_response(stream, "200 OK", content_type, &*asset.data, &headers).await?;
    }
    Ok(())
}

async fn write_json(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &serde_json::Value,
    extra: &[(&str, &str)],
) -> anyhow::Result<()> {
    write_response(stream, status, "application/json", body.to_string(), extra).await?;
    Ok(())
}

/// A request's body of `length` bytes, the part already read with its head first.
async fn read_body(stream: &mut tokio::net::TcpStream, request: &[u8], length: usize) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(length <= MAX_LOGIN_BODY, "a login body exceeds {MAX_LOGIN_BODY} bytes");
    let start = head_end(request)? + 4;
    let mut body = request[start..].to_vec();
    tokio::time::timeout(REQUEST_HEAD_TIMEOUT, async {
        let mut chunk = [0u8; 1024];
        while body.len() < length {
            let read = stream.read(&mut chunk).await?;
            anyhow::ensure!(read > 0, "the login body ended early");
            body.extend_from_slice(&chunk[..read]);
        }
        Ok(())
    })
    .await
    .context("the login body did not arrive within 10 seconds")??;
    body.truncate(length);
    Ok(body)
}

/// The request head as the worker is given it: with the worker's token as its
/// only zDesk cookie, and closing the connection after one response.
///
/// The token is the master's to present, never the browser's. The browser's
/// cookies go through except the logins of this master and of any other on
/// another port, which the worker has no use for, and any `zdesk_session`, which on this host can only be a served
/// gateway's on the same `<label>.zdesk.localhost` — a cookie ignores the port —
/// or a stale one.
///
/// One response per connection, because the master reads only the first request
/// on a connection before handing the rest to the worker. A browser keeping the
/// connection would send its next request past the master's check of its login.
/// A WebSocket upgrade keeps
/// its `Connection` header; it is one request and then the socket.
fn forwarded(request: &[u8], parsed: &ParsedRequest, token: &str) -> anyhow::Result<Vec<u8>> {
    let end = head_end(request)?;
    let head = std::str::from_utf8(&request[..end]).context("HTTP headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let mut out = String::with_capacity(head.len() + 128);
    out.push_str(lines.next().context("missing HTTP request line")?);
    out.push_str("\r\n");
    let mut cookies = Vec::new();
    for line in lines {
        let name = line.split_once(':').map_or(line, |(name, _)| name).trim();
        if name.eq_ignore_ascii_case("cookie") {
            cookies.extend(line.split_once(':').into_iter().flat_map(|(_, value)| value.split(';')).map(str::trim).filter(
                |pair| {
                    let key = pair.split_once('=').map_or(*pair, |(key, _)| key);
                    !pair.is_empty() && !key.starts_with(LOGIN_COOKIE_PREFIX) && key != crate::auth::COOKIE_NAME
                },
            ));
            continue;
        }
        if !parsed.upgrade
            && ["connection", "keep-alive", "proxy-connection"]
                .iter()
                .any(|hop| name.eq_ignore_ascii_case(hop))
        {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    let session = format!("{}={token}", crate::auth::COOKIE_NAME);
    cookies.push(&session);
    out.push_str(&format!("Cookie: {}\r\n", cookies.join("; ")));
    if !parsed.upgrade {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&request[end + 4..]);
    Ok(bytes)
}

struct ParsedRequest {
    method: String,
    host: String,
    target: String,
    cookie: Option<String>,
    origin: Option<String>,
    content_length: usize,
    if_none_match: Option<String>,
    /// Whether it asks to upgrade the connection, as a WebSocket does.
    upgrade: bool,
}

async fn read_request_head(stream: &mut tokio::net::TcpStream) -> anyhow::Result<Option<Vec<u8>>> {
    tokio::time::timeout(REQUEST_HEAD_TIMEOUT, async {
        let mut request = Vec::with_capacity(4096);
        let mut chunk = [0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Ok((!request.is_empty()).then_some(request));
            }
            request.extend_from_slice(&chunk[..read]);
            anyhow::ensure!(request.len() <= MAX_REQUEST_HEAD, "request headers exceed 64 KiB");
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                return Ok(Some(request));
            }
        }
    })
    .await
    .context("request headers did not arrive within 10 seconds")?
}

/// Where the head's closing blank line starts.
fn head_end(request: &[u8]) -> anyhow::Result<usize> {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("incomplete HTTP request headers")
}

fn parse_request(request: &[u8]) -> anyhow::Result<ParsedRequest> {
    let end = head_end(request)?;
    let head = std::str::from_utf8(&request[..end]).context("HTTP headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().context("missing HTTP request line")?;
    let mut pieces = request_line.split_whitespace();
    let method = pieces.next().context("missing HTTP method")?.to_owned();
    let target = pieces.next().context("missing HTTP request target")?.to_owned();
    let version = pieces.next().context("missing HTTP version")?;
    anyhow::ensure!(version.starts_with("HTTP/"), "invalid HTTP version");
    anyhow::ensure!(target.starts_with('/'), "only origin-form HTTP requests are accepted");

    let mut host = None;
    let mut cookies = Vec::new();
    let mut origin = None;
    let mut content_length = 0;
    let mut if_none_match = None;
    let mut upgrade = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            anyhow::bail!("malformed HTTP header");
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            host = Some(value.to_owned());
        } else if name.eq_ignore_ascii_case("cookie") {
            cookies.push(value);
        } else if name.eq_ignore_ascii_case("origin") {
            origin = Some(value.to_owned());
        } else if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().context("invalid Content-Length")?;
        } else if name.eq_ignore_ascii_case("if-none-match") {
            if_none_match = Some(value.to_owned());
        } else if name.eq_ignore_ascii_case("upgrade") {
            upgrade = true;
        }
    }
    Ok(ParsedRequest {
        method,
        host: host.context("request has no Host header")?,
        target,
        cookie: (!cookies.is_empty()).then(|| cookies.join("; ")),
        origin,
        content_length,
        if_none_match,
        upgrade,
    })
}

/// A `Host` header's name, lowercased, when the header names `port`; a browser
/// leaves the port out only for 80.
fn host_at(host: &str, port: u16) -> Option<String> {
    let lowercase = host.trim().to_ascii_lowercase();
    let split = if lowercase.starts_with('[') {
        lowercase.find(']')? + 1
    } else {
        lowercase.rfind(':').unwrap_or(lowercase.len())
    };
    let (name, rest) = lowercase.split_at(split);
    let given = match rest.strip_prefix(':') {
        Some(digits) => digits.parse::<u16>().ok()?,
        None if rest.is_empty() => 80,
        None => return None,
    };
    (given == port).then(|| name.to_owned())
}

async fn write_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: impl AsRef<[u8]>,
    extra: &[(&str, &str)],
) -> std::io::Result<()> {
    let body = body.as_ref();
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tick is not a reason to touch the terminal. A repaint that changes
    /// nothing still erases the display, and a selection made over this screen
    /// does not survive that.
    #[test]
    fn an_unchanged_frame_is_not_written_again() {
        let mut screen = Screen::default();
        let mut out = Vec::new();

        screen.draw(b"first".to_vec(), &mut out).unwrap();
        assert_eq!(out, b"first");

        for _ in 0..4 {
            screen.draw(b"first".to_vec(), &mut out).unwrap();
        }
        assert_eq!(out, b"first", "four idle ticks wrote nothing");

        screen.draw(b"second".to_vec(), &mut out).unwrap();
        assert_eq!(out, b"firstsecond", "a frame that differs is written");

        // Something else — the editor — has had the screen since the last frame.
        screen.invalidate();
        screen.draw(b"second".to_vec(), &mut out).unwrap();
        assert_eq!(out, b"firstsecondsecond");
    }

    /// The key loop, the key bar and the keys page read one table, so it must
    /// bind every action once and no key twice.
    #[test]
    fn every_action_has_one_binding_and_every_key_one_action() {
        let mut seen = Vec::new();
        for binding in BINDINGS {
            assert!(
                BINDINGS.iter().filter(|other| other.action == binding.action).count() == 1,
                "{:?} is bound twice",
                binding.action
            );
            for code in binding.codes {
                assert!(!seen.contains(code), "{code:?} is bound twice");
                seen.push(*code);
                assert_eq!(action_for(*code), Some(binding.action));
            }
        }
        assert_eq!(action_for(KeyCode::Char('z')), None);

        let page = key_lines().join("\n");
        for binding in BINDINGS {
            assert!(page.contains(binding.about), "the keys page lists {:?}: {page}", binding.action);
        }
    }

    /// The list screen at the smallest common terminal: the access panel says who
    /// can reach an instance, and the whole key bar fits on its one line.
    #[test]
    fn the_list_screen_fits_an_80_column_terminal() {
        let every: Vec<_> = BINDINGS.iter().filter(|binding| binding.bar).collect();
        assert!(bar_width(&every) <= 80, "the key bar is {} columns", bar_width(&every));
        assert_eq!(list_bar(80).len(), every.len());

        // Narrower, the instance keys give way and the keys page stays named.
        let narrow = list_bar(40);
        assert!(narrow.len() < every.len());
        assert!(narrow.contains(&("?".to_owned(), "keys")), "{narrow:?}");
        assert!(narrow.contains(&("q".to_owned(), "quit")), "{narrow:?}");

        let options = TuiOptions {
            port: 52380,
            instances_dir: PathBuf::from("/instances"),
        };
        let instances = [InstanceInfo {
            name: "work".to_owned(),
            status: InstanceStatus::Running,
            dir: PathBuf::from("/instances/work"),
            detail: None,
        }];
        let pin = PinState::Open {
            digits: "123456".to_owned(),
            expires_at: "14:32".to_owned(),
            minutes_left: 10,
        };
        let frame = render((80, 24), &options, &pin, &instances, 0, &View::List, "started work").unwrap();
        let frame = String::from_utf8(frame).unwrap();
        assert!(frame.contains("123-456") && !frame.contains('█'), "{frame}");
        assert!(frame.contains("press P for a new PIN · c copies it"), "{frame}");
        assert!(frame.contains("expires 14:32 · 10 min left"), "{frame}");
        assert!(frame.contains("┌ access PIN "), "{frame}");
        assert!(frame.contains("http://work.zdesk.localhost:52380"), "{frame}");
        for (key, label) in list_bar(80) {
            assert!(frame.contains(&key) && frame.contains(&format!(" {label}")), "{key} {label}: {frame}");
        }

        // Too narrow for the keys page and quitting together, the screen still
        // names quitting.
        assert_eq!(list_bar(10), vec![("q".to_owned(), "quit")]);
        let frame = render((10, 24), &options, &pin, &instances, 0, &View::List, "").unwrap();
        let frame = String::from_utf8(frame).unwrap();
        assert!(frame.contains(" quit") && !frame.contains(" keys"), "{frame}");
    }

    /// The PIN is drawn in a colour of its own, apart from its label, and a PIN
    /// that cannot log anyone in says that only new logins are refused.
    #[test]
    fn the_access_panel_sets_the_pin_apart() {
        let open = PinState::Open {
            digits: "123456".to_owned(),
            expires_at: "14:32".to_owned(),
            minutes_left: 10,
        };
        let [heading, ..] = access_rows(&open);
        assert_eq!(
            heading,
            vec![
                ("PIN ".to_owned(), Some(Color::Green), true),
                ("123-456".to_owned(), Some(Color::Magenta), true),
            ]
        );
        for pin in [PinState::Expired, PinState::Withdrawn] {
            let [_, detail, _] = access_rows(&pin);
            assert_eq!(detail[0].0, "no new browser can log in · logged-in ones stay in");
        }
    }

    /// The clipboard write is OSC 52 for the system clipboard, its text in
    /// standard base64, ended by BEL: the form every emulator that takes the
    /// sequence takes.
    #[test]
    fn the_pin_is_copied_through_the_terminal() {
        assert_eq!(dashed("123456"), "123-456");
        assert_eq!(clipboard_write(&dashed("123456")), b"\x1b]52;c;MTIzLTQ1Ng==\x07");
    }

    /// The specs page answers from the config the instance would start on, and
    /// what it says about credentials is never the credentials.
    #[test]
    fn the_specs_page_describes_an_instance_without_printing_its_passwords() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("work");
        std::fs::create_dir(&dir).unwrap();
        let instance = InstanceInfo {
            name: "work".to_owned(),
            status: InstanceStatus::Running,
            dir,
            detail: None,
        };
        std::fs::write(
            instance.config_path(),
            crate::config::vaulted("[branding]\ntext = \"work laptop\"\n\n\
             [[targets]]\nname = \"win\"\nprotocol = \"rdp\"\nhost = \"192.168.1.20\"\n\
             username = \"andrew\"\npassword_ref = \"win11\"\n\
             video_quality = 70\n\n\
             [[targets]]\nname = \"desk\"\nprotocol = \"vnc\"\nsubtype = \"wlshare\"\n\
             host = \"192.168.1.21\"\n"),
        )
        .unwrap();

        let page = describe_instance(&instance, 52380).join("\n");
        assert!(page.contains(&spec("state", "running")), "{page}");
        assert!(page.contains("http://work.zdesk.localhost:52380"), "{page}");
        assert!(page.contains(&spec("shown as", "work laptop")), "{page}");
        assert!(page.contains("target win"), "{page}");
        assert!(page.contains("192.168.1.20:3389"), "the standard port is filled in: {page}");
        assert!(page.contains("account password [passwords].win11"), "{page}");
        assert!(
            page.contains("opus ≤192 kbit/s, adaptive down to 24 kbit/s"),
            "an unset switch is named with the one rate, walk included: {page}"
        );
        assert!(
            page.contains(&spec("render", "the host's graphics pipeline, passed through")),
            "the render plan describes itself: {page}"
        );
        assert!(
            page.contains(&spec("picker", "offers resize, sound, camera, microphone, high quality or adaptive H.264")),
            "what a session's starter chooses is the type's to offer, and the pipeline is passed unasked: {page}"
        );
        assert!(
            page.contains(&spec("picker", "offers resize, sound, camera, microphone, wlshare's VP9 passed through, wlshare's tiles with motion passed through")),
            "a wlshare target's picture is one of its two, the VP9 where none is named: {page}"
        );

        // A config the gateway would refuse says so, instead of a page of
        // defaults for a start that will not happen.
        std::fs::write(instance.config_path(), "[server]\n").unwrap();
        let page = describe_instance(&instance, 52380).join("\n");
        assert!(page.contains("will not start"), "{page}");
        assert!(page.contains("[server]"), "it names what is wrong: {page}");
    }

    /// A clean exit this manager did not request is not a failure, and the
    /// account of it says which it was, so a `pkill` aimed at some other zdesk
    /// is not read as a crash.
    #[cfg(unix)]
    #[test]
    fn an_unrequested_exit_is_only_a_failure_when_the_gateway_says_so() {
        use std::os::unix::process::ExitStatusExt as _;
        let status = |raw: i32| std::process::ExitStatus::from_raw(raw);

        let clean = ExitKind::of(status(0));
        assert_eq!(clean, ExitKind::Clean);
        assert!(clean.is_clean());
        assert!(clean.to_string().contains("exit code 0"), "{clean}");

        let code = ExitKind::of(status(2 << 8));
        assert_eq!(code, ExitKind::Code(2));
        assert!(!code.is_clean());
        assert!(code.to_string().contains("exit code 2"), "{code}");

        let term = ExitKind::of(status(libc::SIGTERM));
        assert_eq!(term, ExitKind::Stopped(libc::SIGTERM));
        assert!(term.is_clean());
        assert!(term.to_string().contains("SIGTERM"), "{term}");
        assert!(ExitKind::of(status(libc::SIGINT)).is_clean());
        assert!(ExitKind::of(status(libc::SIGHUP)).is_clean());

        let segv = ExitKind::of(status(libc::SIGSEGV));
        assert_eq!(segv, ExitKind::Crashed(libc::SIGSEGV));
        assert!(!segv.is_clean());
        assert!(segv.to_string().contains("SIGSEGV"), "{segv}");
        assert!(!ExitKind::of(status(libc::SIGKILL)).is_clean());
    }

    /// Windows says all of it with the exit code: a status the system ended the
    /// process with is a crash however it is spelled, and a code the gateway chose
    /// is that code — `TerminateProcess` included, which names one.
    #[cfg(windows)]
    #[test]
    fn an_unrequested_exit_is_only_a_failure_when_the_gateway_says_so() {
        use std::os::windows::process::ExitStatusExt as _;
        let status = |raw: u32| std::process::ExitStatus::from_raw(raw);

        let clean = ExitKind::of(status(0));
        assert_eq!(clean, ExitKind::Clean);
        assert!(clean.is_clean());
        assert!(clean.to_string().contains("exit code 0"), "{clean}");

        let code = ExitKind::of(status(2));
        assert_eq!(code, ExitKind::Code(2));
        assert!(!code.is_clean());
        assert!(code.to_string().contains("exit code 2"), "{code}");
        assert_eq!(ExitKind::of(status(1)), ExitKind::Code(1), "what TerminateProcess leaves");

        let interrupted = ExitKind::of(status(STATUS_CONTROL_C_EXIT));
        assert_eq!(interrupted, ExitKind::Stopped(STATUS_CONTROL_C_EXIT));
        assert!(interrupted.is_clean());
        assert!(interrupted.to_string().contains("0xC000013A"), "{interrupted}");

        let violation = ExitKind::of(status(0xC000_0005));
        assert_eq!(violation, ExitKind::Crashed(0xC000_0005));
        assert!(!violation.is_clean());
        assert!(violation.to_string().contains("STATUS_ACCESS_VIOLATION"), "{violation}");
        let abort = ExitKind::of(status(0xC000_0409));
        assert!(!abort.is_clean());
        assert!(abort.to_string().contains("abort"), "a panic's end is named: {abort}");
    }

    #[test]
    fn instance_names_are_exactly_one_lowercase_dns_label() {
        for valid in ["one", "work-2", "a"] {
            valid_instance_name(valid).unwrap();
        }
        for invalid in ["", "UPPER", "two.words", "-first", "last-", "has space"] {
            assert!(valid_instance_name(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn request_parsing_separates_host_target_and_cookie() {
        let parsed = parse_request(
            b"GET /api/targets?q=1 HTTP/1.1\r\nHost: Work.zdesk.localhost:52380\r\nCookie: other=1; zdesk_session=secret\r\n\r\n",
        )
        .unwrap();
        assert_eq!(host_at(&parsed.host, 52380).as_deref(), Some("work.zdesk.localhost"));
        assert_eq!(parsed.target, "/api/targets?q=1");
        assert_eq!(parsed.method, "GET");
        assert!(!parsed.upgrade);
        assert_eq!(parsed.cookie.as_deref(), Some("other=1; zdesk_session=secret"));
    }

    /// The worker is given its token by the master, in place of whatever zDesk
    /// cookies the browser sent, and one response per connection unless the
    /// request is an upgrade.
    #[test]
    fn a_forwarded_request_carries_the_token_and_not_the_browsers_logins() {
        let request = b"POST /api/x HTTP/1.1\r\nHost: work.zdesk.localhost:52380\r\nConnection: keep-alive\r\nCookie: zdesk_login_52380=mine; zdesk_login_52381=theirs; theme=dark\r\nCookie: zdesk_session=served\r\nContent-Length: 2\r\n\r\nhi";
        let parsed = parse_request(request).unwrap();
        let given = String::from_utf8(forwarded(request, &parsed, "worker-token").unwrap()).unwrap();
        assert!(given.starts_with("POST /api/x HTTP/1.1\r\nHost: work.zdesk.localhost:52380\r\n"), "{given}");
        assert!(given.contains("\r\nCookie: theme=dark; zdesk_session=worker-token\r\n"), "{given}");
        assert!(!given.contains("mine") && !given.contains("theirs") && !given.contains("served"), "{given}");
        assert!(!given.contains("keep-alive"), "{given}");
        assert!(given.contains("\r\nConnection: close\r\n"), "{given}");
        assert!(given.ends_with("\r\n\r\nhi"), "the body read with the head follows it: {given}");

        let upgrade = b"GET /ws HTTP/1.1\r\nHost: work.zdesk.localhost:52380\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n";
        let parsed = parse_request(upgrade).unwrap();
        assert!(parsed.upgrade);
        let given = String::from_utf8(forwarded(upgrade, &parsed, "worker-token").unwrap()).unwrap();
        assert!(given.contains("\r\nConnection: Upgrade\r\n"), "{given}");
        assert!(!given.contains("Connection: close"), "{given}");
        assert!(given.contains("\r\nCookie: zdesk_session=worker-token\r\n"), "{given}");
    }

    #[test]
    fn a_new_instance_gets_the_worker_config_shape() {
        let temp = tempfile::tempdir().unwrap();
        bootstrap_config(temp.path()).unwrap();
        let text = std::fs::read_to_string(temp.path().join("zdesk.toml")).unwrap();
        assert!(!text.lines().any(|line| line.trim() == "[server]"));
        super::super::check(&text).unwrap();
    }

    /// A path cmd would read as variables — `%TEMP%`, `!x!` — or as operators
    /// reaches the editor exactly as it is.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_editor_gets_the_path_exactly_as_it_is() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("%TEMP% & !PATH! ^ (x)");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("zdesk.toml");
        std::fs::write(&path, "the-right-file").unwrap();

        let output = editor_command("type", &path).output().await.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(stdout.contains("the-right-file"), "{stdout}");
    }

    /// An instance something else serves — another control plane on the same
    /// directory, or a worker started by hand — is refused before any worker is
    /// spawned, and says why.
    #[tokio::test(start_paused = true)]
    async fn an_instance_another_gateway_holds_is_refused_before_spawning() {
        let root = tempfile::tempdir().unwrap();
        let mut supervisor = Supervisor::open(root.path().to_path_buf(), PathBuf::from("no-such-zdesk"))
            .await
            .unwrap();
        supervisor.create("alpha").await.unwrap();
        let _held = super::super::Instance::new(root.path().join("alpha")).claim().await.unwrap();

        let error = supervisor.start("alpha", None).await.unwrap_err();
        assert!(format!("{error:#}").contains("already served by another gateway"), "{error:#}");
        assert_eq!(supervisor.instances()[0].status, InstanceStatus::Stopped);
    }

    #[tokio::test]
    async fn two_subdomains_share_one_port_and_reach_different_gateways() {
        let routes = RouteTable::default();
        let first = fake_gateway("one").await;
        let second = fake_gateway("two").await;
        routes.inner.write().await.extend([
            (
                "one".to_owned(),
                PublishedInstance {
                    status: InstanceStatus::Running,
                    target: Some(RouteTarget {
                        endpoint: first.0.clone(),
                        token: "token-one".to_owned(),
                    }),
                },
            ),
            (
                "two".to_owned(),
                PublishedInstance {
                    status: InstanceStatus::Running,
                    target: Some(RouteTarget {
                        endpoint: second.0.clone(),
                        token: "token-two".to_owned(),
                    }),
                },
            ),
        ]);
        let port = free_port();
        let access = Access::open(port);
        let login = format!("{}={}", access.cookie_name(), access.log_in(&pin_of(&access)).unwrap());
        let router = SharedPort::bind(port, routes, access).await.unwrap();

        let first_response = request(router.port(), "one.zdesk.localhost", Some(&login)).await;
        let second_response = request(router.port(), "two.zdesk.localhost", Some(&login)).await;
        assert_eq!(body_of(&first_response).lines().next(), Some("one"), "{first_response}");
        assert_eq!(body_of(&second_response).lines().next(), Some("two"), "{second_response}");
        assert!(first_response.contains("zdesk_session=token-one"), "the worker got its own token: {first_response}");
        assert!(second_response.contains("zdesk_session=token-two"), "{second_response}");
        first.1.await.unwrap();
        second.1.await.unwrap();
    }

    /// A browser without a login is asked for the PIN where it is: every origin's
    /// page is the PIN form, and an instance origin gives nothing of the
    /// instance — no API, and never a worker's token.
    #[tokio::test]
    async fn a_browser_without_a_login_is_asked_for_the_pin_where_it_is() {
        let routes = RouteTable::default();
        routes.inner.write().await.insert(
            "one".to_owned(),
            PublishedInstance {
                status: InstanceStatus::Running,
                target: Some(RouteTarget {
                    endpoint: transport::endpoint(Path::new("/no/such/instance")),
                    token: "launch-token".to_owned(),
                }),
            },
        );
        let port = free_port();
        let router = SharedPort::bind(port, routes, Access::open(port)).await.unwrap();

        for host in ["zdesk.localhost", "one.zdesk.localhost", "nothing.zdesk.localhost"] {
            for path in ["/", "/display/2?x=1"] {
                let page = get(port, host, path, Some("zdesk_session=launch-token")).await;
                assert!(page.starts_with("HTTP/1.1 200 OK"), "{host}{path}: {page}");
                assert!(page.contains("Content-Type: text/html; charset=utf-8\r\n"), "{host}{path}: {page}");
                assert!(page.contains("<div id=\"root\">"), "the bundle's document: {host}{path}: {page}");
                assert!(page.contains("Cross-Origin-Embedder-Policy: require-corp\r\n"), "isolated as a gateway's: {page}");
                assert!(!page.contains("launch-token") && !page.contains("Set-Cookie"), "{host}{path}: {page}");
                assert!(!page.contains("Location"), "never sent elsewhere: {host}{path}: {page}");
            }
            let status = get(port, host, "/api/auth/status", Some("zdesk_session=launch-token")).await;
            assert_eq!(body_of(&status), "{\"authenticated\":false,\"login\":\"pin\",\"problem\":null}", "{host}: {status}");
            let config = get(port, host, "/api/config", Some("zdesk_session=launch-token")).await;
            assert!(body_of(&config).contains(&format!("\"branding\":\"{DEFAULT_BRANDING}\"")), "{host}: {config}");
            for path in ["/api/targets", "/api/instances", "/ws", "/ws/audio"] {
                let refused = get(port, host, path, Some("zdesk_session=launch-token")).await;
                assert!(refused.starts_with("HTTP/1.1 401 Unauthorized"), "{host}{path}: {refused}");
                let version = format!("\r\nX-Zdesk-Version: {}\r\n", env!("CARGO_PKG_VERSION"));
                assert!(refused.contains(&version), "{host}{path}: {refused}");
            }
        }
        drop(router);
    }

    /// A bare loopback name is sent to `zdesk.localhost`, keeping the path, and
    /// never shown the PIN form: a login set there would reach every local server.
    #[tokio::test]
    async fn a_bare_loopback_name_is_sent_to_the_masters_own() {
        let port = free_port();
        let router = SharedPort::bind(port, RouteTable::default(), Access::open(port)).await.unwrap();
        for host in ["localhost", "127.0.0.1", "[::1]"] {
            let response = send(port, &format!("GET /display/2?x=1 HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n")).await;
            assert!(response.starts_with("HTTP/1.1 307 Temporary Redirect"), "{host}: {response}");
            assert!(
                response.contains(&format!("Location: http://zdesk.localhost:{port}/display/2?x=1\r\n")),
                "{host}: {response}"
            );
            assert!(!response.contains("Set-Cookie"), "{host}: {response}");
        }
        drop(router);
    }

    #[test]
    fn a_host_counts_only_at_the_masters_port() {
        assert_eq!(host_at("[::1]:52380", 52380).as_deref(), Some("[::1]"));
        assert_eq!(host_at("zdesk.localhost", 80).as_deref(), Some("zdesk.localhost"));
        for other in ["zdesk.localhost", "zdesk.localhost:8080", "zdesk.localhost:x", "[::1]x", "::1"] {
            assert_eq!(host_at(other, 52380), None, "{other}");
        }
    }

    /// Only `zdesk.localhost` and its subdomains at the master's own port are
    /// answered, so a tunnel or a forward to another port is refused outright.
    #[tokio::test]
    async fn any_other_host_or_port_is_blocked() {
        let port = free_port();
        let router = SharedPort::bind(port, RouteTable::default(), Access::open(port)).await.unwrap();
        for host in [
            "example.trycloudflare.com".to_owned(),
            format!("zdesk.localhost.example.com:{port}"),
            format!("evil.localhost:{port}"),
            "zdesk.localhost".to_owned(),
            format!("zdesk.localhost:{}", port.wrapping_add(1)),
            "work.zdesk.localhost:8080".to_owned(),
            "localhost:8080".to_owned(),
        ] {
            let response = send(port, &format!("GET /api/auth/status HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
            assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{host}: {response}");
            assert!(!response.contains("Set-Cookie") && !response.contains("Location"), "{host}: {response}");
        }
        drop(router);
    }

    /// The page's PIN logs the browser in, on the landing origin or an instance's,
    /// for the landing origin's domain; a post from another origin is not taken,
    /// and does not count against the PIN.
    #[tokio::test]
    async fn the_pin_logs_a_browser_in_from_its_own_origin_only() {
        let port = free_port();
        let access = Access::open(port);
        let _router = SharedPort::bind(port, RouteTable::default(), access.clone()).await.unwrap();
        let post = |host: &str, origin: String, pin: &str| {
            let body = format!("{{\"pin\":\"{pin}\"}}");
            format!(
                "POST {PIN_PATH} HTTP/1.1\r\nHost: {host}:{port}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };

        let mut wrong_entries = 0;
        for host in ["zdesk.localhost", "one.zdesk.localhost"] {
            let pin = pin_of(&access);
            for origin in ["http://evil.example".to_owned(), format!("http://{host}"), format!("http://other.zdesk.localhost:{port}")] {
                let response = send(port, &post(host, origin.clone(), "000000")).await;
                assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{host} from {origin}: {response}");
                assert!(response.contains("its own origin"), "{response}");
            }

            let wrong = if pin == "000000" { "000001" } else { "000000" };
            wrong_entries += 1;
            let response = send(port, &post(host, format!("http://{host}:{port}"), wrong)).await;
            assert!(response.starts_with("HTTP/1.1 401 Unauthorized"), "{host}: {response}");
            let left = MAX_WRONG - wrong_entries;
            assert!(response.contains(&format!("{left} more wrong entries")), "only this one counted: {host}: {response}");

            let response = send(port, &post(host, format!("http://{host}:{port}"), &pin)).await;
            assert!(response.starts_with("HTTP/1.1 200 OK"), "{host}: {response}");
            let cookie = response
                .lines()
                .find_map(|line| line.strip_prefix("Set-Cookie: "))
                .expect("a login cookie");
            assert!(cookie.starts_with(&format!("zdesk_login_{port}=")), "{cookie}");
            assert!(cookie.contains("; Domain=zdesk.localhost;") && cookie.contains("; HttpOnly; SameSite=Lax;"), "{cookie}");
            let login = cookie.split(';').next().unwrap();

            let status = get(port, "zdesk.localhost", "/api/auth/status", Some(login)).await;
            assert!(body_of(&status).starts_with("{\"authenticated\":true,\"login\":\"pin\","), "{host}: {status}");
            let instances = get(port, "zdesk.localhost", "/api/instances", Some(login)).await;
            assert!(instances.starts_with("HTTP/1.1 200 OK") && body_of(&instances) == "[]", "{host}: {instances}");
        }
    }

    /// A login, wherever its PIN was entered, lists the instances on the landing
    /// origin and reaches each instance's worker with the worker's token in its
    /// place.
    #[tokio::test]
    async fn a_login_reaches_the_landing_origin_and_every_instance() {
        let routes = RouteTable::default();
        let worker = fake_gateway("one").await;
        routes.inner.write().await.insert(
            "one".to_owned(),
            PublishedInstance {
                status: InstanceStatus::Running,
                target: Some(RouteTarget {
                    endpoint: worker.0.clone(),
                    token: "token-one".to_owned(),
                }),
            },
        );
        let port = free_port();
        let access = Access::open(port);
        let login = format!("{}={}", access.cookie_name(), access.log_in(&pin_of(&access)).unwrap());
        let _router = SharedPort::bind(port, routes, access).await.unwrap();

        let listed = get(port, "zdesk.localhost", "/api/instances", Some(&login)).await;
        assert_eq!(
            body_of(&listed),
            format!("[{{\"name\":\"one\",\"open\":\"http://one.zdesk.localhost:{port}\",\"state\":\"running\"}}]"),
            "{listed}"
        );
        let unknown = get(port, "two.zdesk.localhost", "/", Some(&login)).await;
        assert!(unknown.starts_with("HTTP/1.1 404 Not Found"), "{unknown}");

        let reached = request(port, "one.zdesk.localhost", Some(&login)).await;
        let seen = body_of(&reached);
        assert_eq!(seen.lines().next(), Some("one"), "{reached}");
        assert!(seen.contains("Cookie: zdesk_session=token-one\r\n"), "{seen}");
        assert!(seen.contains("Connection: close\r\n"), "{seen}");
        worker.1.await.unwrap();
    }

    fn pin_of(access: &Access) -> String {
        let PinState::Open { digits, .. } = access.pin() else {
            panic!("a new PIN is open");
        };
        digits
    }

    fn body_of(response: &str) -> &str {
        response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    /// A port nothing is listening on, found by taking one and letting it go —
    /// `free_port` in `src/server.rs`, for its reason. A test needs a port the rest
    /// of the machine is not using; the control plane needs a port its operator
    /// chose, and asking the kernel for one is not a thing it may do.
    fn free_port() -> u16 {
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// The master password is the keychain's, made once: a second control plane
    /// finds the one the first generated, and a config made under another
    /// account's is told as one.
    #[test]
    fn the_master_password_is_generated_once_and_kept_in_the_keychain() {
        let _store = crate::keychain::empty_test_store();
        let mut first = Master::default();
        let generated = first.get().unwrap().clone();
        assert_eq!(generated.as_str().len(), 44, "32 random bytes, base64");
        assert_eq!(first.get().unwrap().as_str(), generated.as_str());
        let mut second = Master::default();
        assert_eq!(second.get().unwrap().as_str(), generated.as_str());
        assert_eq!(crate::keychain::load().unwrap().unwrap().as_str(), generated.as_str());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zdesk.toml");
        let target = "[[targets]]\nname = \"w\"\nprotocol = \"rdp\"\nhost = \"h\"\nusername = \"u\"\npassword_ref = \"w\"\n";
        let foreign = MasterPassword::new("another account's".to_owned()).unwrap();
        let (_, orphan) = Key::create(&foreign).unwrap();
        let value = orphan.encrypt("hunter2").unwrap();
        std::fs::write(&path, format!("{target}[passwords]\nw = \"{value}\"\n")).unwrap();
        let err = second.key_for(&path).unwrap_err();
        assert!(format!("{err:#}").contains("no [master]"), "{err:#}");

        crate::vault::init_file(&path, &foreign).unwrap();
        crate::vault::set_file(&path, "w", "hunter2", &foreign).unwrap();
        let err = second.key_for(&path).unwrap_err();
        assert!(format!("{err:#}").contains("another account's master password"), "{err:#}");

        std::fs::write(&path, target).unwrap();
        crate::vault::init_file(&path, &generated).unwrap();
        crate::vault::set_file(&path, "w", "hunter2", &generated).unwrap();
        let key = second.key_for(&path).unwrap().expect("a config naming passwords has a key");
        let mut file = ConfigFile::parse_with(&std::fs::read_to_string(&path).unwrap(), Audience::Worker).unwrap();
        file.unlock(Some(&key)).unwrap();
        assert_eq!(file.targets[0].password, "hunter2");
    }

    /// A master left holding one family of the shared port is not something to
    /// start beside: the browser picks the family, so half its page loads would
    /// reach the old process and its old instance workers.
    #[tokio::test]
    async fn a_port_half_taken_by_an_earlier_master_refuses_the_start() {
        let port = free_port();
        // The earlier master is simulated by holding one family, so a machine with
        // IPv6 off has no half to take: `bind_all` skips `::1` there and the start
        // it would refuse is one that legitimately succeeds. Any other bind error is
        // still the test failing.
        let squatter = match std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrNotAvailable => return,
            Err(error) => panic!("cannot hold the v6 half of {port}: {error}"),
        };

        let Err(error) = SharedPort::bind(port, RouteTable::default(), Access::open(port)).await else {
            panic!("the v6 half of this port is already served");
        };
        let text = format!("{error:#}");
        assert!(text.contains("already in use"), "{text}");
        assert!(text.contains(&port.to_string()), "it must name the port: {text}");

        // And the refusal took nothing: the v4 half it bound on the way through is
        // released, so a retry after stopping the other master works.
        drop(squatter);
        SharedPort::bind(port, RouteTable::default(), Access::open(port))
            .await
            .expect("the refused attempt must not have kept a socket");
    }

    /// The port is typed into a browser, so there is no code path that lets the
    /// kernel choose it — including the one the tests take.
    #[tokio::test]
    async fn an_ephemeral_port_is_not_something_the_control_plane_can_be_asked_for() {
        let Err(error) = SharedPort::bind(0, RouteTable::default(), Access::open(0)).await else {
            panic!("port 0 is a control plane nobody can be told how to reach");
        };
        assert!(format!("{error:#}").contains("a browser can be told"), "{error:#}");
    }

    /// A worker's endpoint answering one request with `body` and, on the next
    /// lines, the request head it was given, over the transport a real worker
    /// listens on.
    async fn fake_gateway(
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = transport::endpoint(directory.path());
        let claim = super::super::Instance::new(directory.path()).claim().await.unwrap();
        let mut listener = transport::WorkerListener::bind(&endpoint, &claim).unwrap();
        let task = tokio::spawn(async move {
            let _claim = claim;
            let (mut stream, _) = axum::serve::Listener::accept(&mut listener).await;
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let read = stream.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = format!("{body}\n{}", String::from_utf8_lossy(&request));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (endpoint, task, directory)
    }

    async fn request(port: u16, host: &str, cookie: Option<&str>) -> String {
        get(port, host, "/", cookie).await
    }

    async fn get(port: u16, host: &str, path: &str, cookie: Option<&str>) -> String {
        let cookie = cookie.map_or(String::new(), |cookie| format!("Cookie: {cookie}\r\n"));
        send(port, &format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{cookie}Connection: close\r\n\r\n")).await
    }

    async fn send(port: u16, raw: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        stream.write_all(raw.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }
}
