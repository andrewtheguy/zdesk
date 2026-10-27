//! Who may use the control plane's port: a browser that has entered the PIN the
//! TUI shows.
//!
//! The master draws a six-digit PIN when it starts and shows it in the TUI's
//! access panel, so the boundary is whoever can see the terminal the TUI runs in,
//! not the machine. A browser that enters it gets a login: a random ID the master
//! keeps in memory and hands back as a cookie named for the master's port,
//! `zdesk_login_<port>`. The PIN
//! guards only new logins. A login outlives the PIN it was made with — its
//! expiry, a new draw, a withdrawal — and every restart of every instance, and
//! ends only with the master, so quitting the TUI is the one way to log out.
//!
//! A PIN takes [`MAX_WRONG`] wrong entries and is then withdrawn: every entry is
//! refused until the person at the terminal draws a new one. Guessing is limited
//! to those few tries per PIN drawn, however fast a local process can post. Such a
//! process can force a new draw, but it never gets in.
//!
//! The PIN is entered on the landing origin, `zdesk.localhost`, and the login is
//! set for that domain, so one login reaches every instance subdomain: an
//! instance origin sends a browser without one to the landing page, which sends
//! it back once the PIN is in. The cookie is `SameSite=Lax`, not `Strict`:
//! WebKit takes `zdesk.localhost` and its subdomains for different sites, and
//! keeps a `Strict` cookie off the navigation from an instance origin to the
//! landing page and back. `Lax` adds only a top-level navigation from another
//! site arriving logged in: a page the user sees and that site cannot read. The
//! site's own `POST`s and sockets are still not sent the cookie.
//!
//! A cookie is scoped by host and ignores the port, so where the login is set
//! decides who else is sent it. The master sets it only for `zdesk.localhost` and
//! its subdomains, never on `localhost` or a numeric loopback, which every local
//! development server shares: it answers those names with a redirect to
//! `zdesk.localhost`. The name carries the port, so two control planes on one
//! machine keep separate logins rather than overwriting each other's.
//!
//! What remains, and is accepted, is anything else serving `zdesk.localhost` or
//! one of its subdomains on another port, a served gateway sent to
//! `<label>.zdesk.localhost` among them. `SameSite` does not stop it: the page
//! such a listener serves is same-site with the master, so its next request to
//! itself carries the login. Any site can send the browser there, so another local user
//! who runs such a listener and gets a link followed takes the login and can
//! replay it past the PIN. Such a listener can also set a cookie of the same name
//! for the domain, which logs the browser out but never in. No browser offers a
//! cookie bound to a port; a host nobody else can name, drawn per run, would close
//! it at the cost of the TUI's stable addresses.
//!
//! The browser never holds a worker's token: the master adds it to each request
//! it forwards. So a browser presents one of two credentials anywhere — this PIN
//! on the TUI's port, or a served gateway's username and password — and the TUI
//! sets no `zdesk_session` cookie to collide with a served gateway's login on a
//! shared `<label>.zdesk.localhost`.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use rand::RngExt as _;
use tokio::time::Instant;

use super::manager::MASTER_HOST;
use crate::auth::secrets_match;

/// The start of the master's login cookie's name, which ends in its port: set
/// for the landing origin's domain, so every instance origin is sent it too.
/// Never `zdesk_session`, which is a served gateway's.
pub const LOGIN_COOKIE_PREFIX: &str = "zdesk_login_";
/// How long a PIN logs browsers in after it is drawn.
pub const PIN_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// Wrong entries a PIN takes before it is withdrawn.
pub const MAX_WRONG: u32 = 5;

/// The PIN and the logins it has made, shared by the TUI and the router.
#[derive(Clone)]
pub struct Access {
    state: Arc<Mutex<State>>,
    /// The login cookie's name, for this master's port.
    cookie: Arc<str>,
}

struct State {
    pin: Pin,
    logins: Vec<String>,
}

struct Pin {
    digits: String,
    expires: Instant,
    /// The local clock time it expires at, written when it is drawn so the panel
    /// does not tick over between two frames of the same minute.
    expires_at: String,
    wrong: u32,
}

impl Pin {
    fn draw() -> Self {
        let expires_at = jiff::Zoned::now()
            .checked_add(PIN_LIFETIME)
            .map_or_else(|_| "?".to_owned(), |time| time.strftime("%H:%M").to_string());
        Self {
            digits: format!("{:06}", rand::rng().random_range(0..1_000_000u32)),
            expires: Instant::now() + PIN_LIFETIME,
            expires_at,
            wrong: 0,
        }
    }
}

/// The PIN as the TUI's access panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinState {
    Open {
        digits: String,
        expires_at: String,
        /// Whole minutes left, rounded up, so the last minute reads as one.
        minutes_left: u64,
    },
    Expired,
    Withdrawn,
}

/// Why an entry logged nobody in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not six digits, so it cannot be the PIN and is not counted against it.
    NotSixDigits,
    Wrong { left: u32 },
    Withdrawn,
    Expired,
}

impl Refusal {
    pub fn message(self) -> String {
        match self {
            Self::NotSixDigits => "A PIN is six digits.".to_owned(),
            Self::Wrong { left } => format!(
                "That is not the PIN. {left} more wrong {} and it is withdrawn.",
                if left == 1 { "entry" } else { "entries" }
            ),
            Self::Withdrawn => format!(
                "The PIN was withdrawn after {MAX_WRONG} wrong entries. Press P in the zDesk TUI to draw a new one."
            ),
            Self::Expired => "The PIN has expired. Press P in the zDesk TUI to draw a new one.".to_owned(),
        }
    }
}

impl Access {
    /// The access of the control plane on `port`, opened with its first PIN drawn
    /// and nobody logged in.
    pub fn open(port: u16) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                pin: Pin::draw(),
                logins: Vec::new(),
            })),
            cookie: format!("{LOGIN_COOKIE_PREFIX}{port}").into(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// The login cookie's name.
    pub fn cookie_name(&self) -> &str {
        &self.cookie
    }

    /// The `Set-Cookie` value for `login`, for the landing origin and every
    /// instance subdomain under it.
    pub fn set_cookie(&self, login: &str) -> String {
        format!("{}={login}; Domain={MASTER_HOST}; HttpOnly; SameSite=Lax; Path=/", self.cookie)
    }

    /// Replace the PIN with a new one whose time starts now. Logins already made
    /// are kept.
    pub fn draw(&self) {
        self.state().pin = Pin::draw();
    }

    pub fn pin(&self) -> PinState {
        let state = self.state();
        let pin = &state.pin;
        if pin.wrong >= MAX_WRONG {
            return PinState::Withdrawn;
        }
        let left = pin.expires.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return PinState::Expired;
        }
        PinState::Open {
            digits: pin.digits.clone(),
            expires_at: pin.expires_at.clone(),
            minutes_left: left.as_secs().div_ceil(60),
        }
    }

    /// Log a browser in with `entry`, returning its new login.
    pub fn log_in(&self, entry: &str) -> Result<String, Refusal> {
        let entry = entry.trim();
        if entry.len() != 6 || !entry.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Refusal::NotSixDigits);
        }
        let mut state = self.state();
        if state.pin.wrong >= MAX_WRONG {
            return Err(Refusal::Withdrawn);
        }
        if Instant::now() >= state.pin.expires {
            return Err(Refusal::Expired);
        }
        if !secrets_match(&state.pin.digits, entry) {
            state.pin.wrong += 1;
            return Err(match MAX_WRONG - state.pin.wrong {
                0 => Refusal::Withdrawn,
                left => Refusal::Wrong { left },
            });
        }
        let login = secret();
        state.logins.push(login.clone());
        Ok(login)
    }

    /// The login a request's `Cookie` header carries, if it is one this master made.
    pub fn login_in(&self, cookie: Option<&str>) -> Option<String> {
        let state = self.state();
        cookie_values(cookie, &self.cookie).find_map(|presented| {
            state
                .logins
                .iter()
                .find(|login| secrets_match(login, presented))
                .cloned()
        })
    }
}

/// 32 random bytes, base64url: a login.
fn secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The values of every cookie called `name` in a `Cookie` header.
pub fn cookie_values<'a>(cookie: Option<&'a str>, name: &'a str) -> impl Iterator<Item = &'a str> {
    cookie
        .into_iter()
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(move |pair| {
            pair.trim()
                .split_once('=')
                .and_then(|(key, value)| (key == name).then_some(value))
        })
}

/// What the page's PIN form says before any entry, when no entry could succeed.
pub fn pin_problem(state: &PinState) -> Option<String> {
    match state {
        PinState::Open { .. } => None,
        PinState::Expired => Some(Refusal::Expired.message()),
        PinState::Withdrawn => Some(Refusal::Withdrawn.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digits(access: &Access) -> String {
        let PinState::Open { digits, .. } = access.pin() else {
            panic!("a new PIN is open: {:?}", access.pin());
        };
        digits
    }

    /// Something other than the PIN, guaranteed.
    fn not(pin: &str) -> String {
        if pin == "000000" { "000001" } else { "000000" }.to_owned()
    }

    #[test]
    fn a_pin_is_six_digits_and_logs_a_browser_in() {
        let access = Access::open(52380);
        let pin = digits(&access);
        assert_eq!(pin.len(), 6);
        assert!(pin.bytes().all(|byte| byte.is_ascii_digit()), "{pin}");

        let login = access.log_in(&pin).unwrap();
        let cookie = format!("other=1; zdesk_login_52380={login}");
        assert_eq!(access.login_in(Some(&cookie)).as_deref(), Some(login.as_str()));
        assert_eq!(access.login_in(Some("zdesk_login_52380=forged")), None);
        assert_eq!(access.login_in(Some(&format!("zdesk_session={login}"))), None, "a served gateway's cookie is not a login");
        assert_eq!(
            access.login_in(Some(&format!("zdesk_login_52381={login}"))),
            None,
            "the login of the control plane on another port is that one's"
        );
        assert_eq!(access.set_cookie(&login), format!("zdesk_login_52380={login}; Domain=zdesk.localhost; HttpOnly; SameSite=Lax; Path=/"));
        assert_eq!(access.login_in(None), None);
    }

    /// The PIN takes `MAX_WRONG` wrong entries and then refuses even itself until
    /// a new one is drawn. An entry that is not six digits cannot be the PIN, so
    /// it costs nothing.
    #[test]
    fn wrong_entries_withdraw_the_pin_until_a_new_one_is_drawn() {
        let access = Access::open(52380);
        let pin = digits(&access);
        assert_eq!(access.log_in("12345"), Err(Refusal::NotSixDigits));
        assert_eq!(access.log_in("abcdef"), Err(Refusal::NotSixDigits));
        for left in (1..MAX_WRONG).rev() {
            assert_eq!(access.log_in(&not(&pin)), Err(Refusal::Wrong { left }));
        }
        assert_eq!(access.log_in(&not(&pin)), Err(Refusal::Withdrawn));
        assert_eq!(access.pin(), PinState::Withdrawn);
        assert_eq!(access.log_in(&pin), Err(Refusal::Withdrawn), "not even the PIN itself");

        access.draw();
        let pin = digits(&access);
        access.log_in(&pin).expect("a new PIN starts with every try");
    }

    /// The PIN guards only new logins: a browser that has logged in stays logged
    /// in after the PIN expires, is drawn again or is withdrawn.
    #[tokio::test(start_paused = true)]
    async fn a_login_outlives_its_pin() {
        let access = Access::open(52380);
        let login = access.log_in(&digits(&access)).unwrap();
        let cookie = format!("zdesk_login_52380={login}");

        let PinState::Open { minutes_left, .. } = access.pin() else { unreachable!() };
        assert_eq!(minutes_left, PIN_LIFETIME.as_secs() / 60);
        tokio::time::advance(PIN_LIFETIME - Duration::from_secs(30)).await;
        let PinState::Open { minutes_left, .. } = access.pin() else { unreachable!() };
        assert_eq!(minutes_left, 1, "the last half minute reads as one");

        tokio::time::advance(Duration::from_secs(30)).await;
        let pin = {
            let state = access.state();
            state.pin.digits.clone()
        };
        assert_eq!(access.pin(), PinState::Expired);
        assert_eq!(access.log_in(&pin), Err(Refusal::Expired));
        assert!(access.login_in(Some(&cookie)).is_some());

        access.draw();
        for _ in 0..MAX_WRONG {
            let _ = access.log_in(&not(&digits(&access)));
        }
        assert_eq!(access.pin(), PinState::Withdrawn);
        assert!(access.login_in(Some(&cookie)).is_some());
    }
}
