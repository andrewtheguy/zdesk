//! Who may use the control plane's port: a browser that has entered the PIN the
//! TUI shows.
//!
//! The master draws a six-digit PIN when it starts and shows it in the TUI's
//! access panel, so the boundary is whoever can see the terminal the TUI runs in,
//! not the machine. A browser that enters it gets a login: a random ID the master
//! keeps in memory and hands back as the host-only [`LOGIN_COOKIE`]. The PIN
//! guards only new logins. A login outlives the PIN it was made with — its
//! expiry, a new draw, a withdrawal — and every restart of every instance, and
//! ends only with the master, so quitting the TUI is the one way to log out.
//!
//! A PIN takes [`MAX_WRONG`] wrong entries and is then withdrawn: every entry is
//! refused until the person at the terminal draws a new one. Guessing is limited
//! to those few tries per PIN drawn, however fast a local process can post. Such a
//! process can force a new draw, but it never gets in.
//!
//! A login made on one origin reaches another through a handoff rather than a
//! cookie shared across subdomains: the landing page's link to an instance mints a
//! one-time code for that instance, and the instance's origin exchanges it for the
//! same login under its own host-only cookie.
//!
//! The browser never holds a worker's token: the master adds it to each request
//! it forwards. So a browser presents one of two credentials anywhere — this PIN
//! on the TUI's port, or a served gateway's username and password — and the TUI
//! sets no `zdesk_session` cookie to collide with a served gateway's login on a
//! shared `<label>.zdesk.localhost`, since a cookie ignores the port.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use rand::RngExt as _;
use tokio::time::Instant;

use crate::auth::secrets_match;

/// The master's login cookie, on the landing origin and on each instance origin
/// a login has reached. Never `zdesk_session`, which is a served gateway's.
pub const LOGIN_COOKIE: &str = "zdesk_login";
/// How long a PIN logs browsers in after it is drawn.
pub const PIN_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// Wrong entries a PIN takes before it is withdrawn.
pub const MAX_WRONG: u32 = 5;
/// How long a handoff code waits to be exchanged. It is followed by a redirect,
/// so it is used within a second or not at all.
const HANDOFF_LIFETIME: Duration = Duration::from_secs(30);

/// The PIN, the logins it has made and the handoffs in flight, shared by the TUI
/// and the router.
#[derive(Clone)]
pub struct Access(Arc<Mutex<State>>);

struct State {
    pin: Pin,
    logins: Vec<String>,
    handoffs: Vec<Handoff>,
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

struct Handoff {
    code: String,
    login: String,
    instance: String,
    expires: Instant,
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
    /// A control plane's access, opened with its first PIN drawn and nobody
    /// logged in.
    pub fn open() -> Self {
        Self(Arc::new(Mutex::new(State {
            pin: Pin::draw(),
            logins: Vec::new(),
            handoffs: Vec::new(),
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap()
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
        cookie_values(cookie, LOGIN_COOKIE).find_map(|presented| {
            state
                .logins
                .iter()
                .find(|login| secrets_match(login, presented))
                .cloned()
        })
    }

    /// A one-time code that carries `login` to `instance`'s origin.
    pub fn hand_off(&self, login: &str, instance: &str) -> String {
        let mut state = self.state();
        let now = Instant::now();
        state.handoffs.retain(|handoff| handoff.expires > now);
        let code = secret();
        state.handoffs.push(Handoff {
            code: code.clone(),
            login: login.to_owned(),
            instance: instance.to_owned(),
            expires: now + HANDOFF_LIFETIME,
        });
        code
    }

    /// Exchange a handoff code on `instance`'s origin for the login it carries.
    /// A code is spent by its first use, whatever that use's outcome, and is good
    /// only for the instance it was minted for.
    pub fn take_handoff(&self, code: &str, instance: &str) -> Option<String> {
        let mut state = self.state();
        let index = state
            .handoffs
            .iter()
            .position(|handoff| secrets_match(&handoff.code, code))?;
        let handoff = state.handoffs.swap_remove(index);
        (handoff.instance == instance && handoff.expires > Instant::now()).then_some(handoff.login)
    }
}

/// 32 random bytes, base64url: a login or a handoff code.
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

/// The `Set-Cookie` value for a login on the origin it is sent from.
pub fn login_cookie(login: &str) -> String {
    format!("{LOGIN_COOKIE}={login}; HttpOnly; SameSite=Strict; Path=/")
}

/// The page that asks for the PIN, with what went wrong last if anything did. Its
/// form posts back to the address it was served at, which the master takes as the
/// entry and then redirects to.
pub fn pin_page(problem: Option<&str>) -> String {
    let problem = problem.map_or(String::new(), |text| format!("<p class=\"problem\" role=\"alert\">{text}</p>"));
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>zDesk login</title><style>body{{font:16px system-ui;max-width:420px;margin:4rem auto;padding:0 1rem;background:#101014;color:#eee}}input{{font:32px ui-monospace,monospace;letter-spacing:.3em;text-indent:.3em;text-align:center;width:11ch;padding:.4rem .6rem;background:#1c1c22;color:#eee;border:1px solid #444;border-radius:4px}}button{{font:18px system-ui;margin-left:.6rem;padding:.7rem 1.2rem;vertical-align:top}}.problem{{color:#ff8a80}}p{{color:#bbb}}</style></head><body><h1>zDesk</h1><p>Enter the PIN shown in the access panel of the zDesk TUI.</p>{problem}<form method=\"post\"><label for=\"pin\">PIN</label><br><input id=\"pin\" name=\"pin\" inputmode=\"numeric\" autocomplete=\"one-time-code\" pattern=\"[0-9]{{6}}\" maxlength=\"6\" required autofocus><button type=\"submit\">Log in</button></form></body></html>"
    )
}

/// What the PIN page says before any entry, when no entry could succeed.
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
        let access = Access::open();
        let pin = digits(&access);
        assert_eq!(pin.len(), 6);
        assert!(pin.bytes().all(|byte| byte.is_ascii_digit()), "{pin}");

        let login = access.log_in(&pin).unwrap();
        let cookie = format!("other=1; {LOGIN_COOKIE}={login}");
        assert_eq!(access.login_in(Some(&cookie)).as_deref(), Some(login.as_str()));
        assert_eq!(access.login_in(Some(&format!("{LOGIN_COOKIE}=forged"))), None);
        assert_eq!(access.login_in(Some(&format!("zdesk_session={login}"))), None, "a served gateway's cookie is not a login");
        assert_eq!(access.login_in(None), None);
    }

    /// The PIN takes `MAX_WRONG` wrong entries and then refuses even itself until
    /// a new one is drawn. An entry that is not six digits cannot be the PIN, so
    /// it costs nothing.
    #[test]
    fn wrong_entries_withdraw_the_pin_until_a_new_one_is_drawn() {
        let access = Access::open();
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
        let access = Access::open();
        let login = access.log_in(&digits(&access)).unwrap();
        let cookie = format!("{LOGIN_COOKIE}={login}");

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

    /// A handoff code carries a login to the one instance it was minted for, once,
    /// and not after its time.
    #[tokio::test(start_paused = true)]
    async fn a_handoff_is_spent_once_and_only_on_its_instance() {
        let access = Access::open();
        let login = access.log_in(&digits(&access)).unwrap();

        let code = access.hand_off(&login, "work");
        assert_eq!(access.take_handoff(&code, "work").as_deref(), Some(login.as_str()));
        assert_eq!(access.take_handoff(&code, "work"), None, "spent");

        let code = access.hand_off(&login, "work");
        assert_eq!(access.take_handoff(&code, "home"), None, "another instance");
        assert_eq!(access.take_handoff(&code, "work"), None, "spent by the wrong use too");

        let code = access.hand_off(&login, "work");
        tokio::time::advance(HANDOFF_LIFETIME).await;
        assert_eq!(access.take_handoff(&code, "work"), None, "expired");
    }
}
