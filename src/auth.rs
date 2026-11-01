//! Who may talk to this gateway: the web login for a browser, or one minted
//! token for the client a worker was started by.
//!
//! A process has exactly one of them ([`GatewayAuth`], chosen at startup), which
//! is what keeps each mode free of the other's machinery.
//!
//! **The login.** `[server].site_username` names the one user, and
//! `[server].site_passwd_hash` holds a one-way hash of that user's password, keyed
//! from the config's master key (see [`crate::vault`]). `/api/auth/login` verifies a
//! username/password pair against them and mints a session token, and everything
//! guarded (the rest of `/api/*`, `/ws`) requires that token in the cookie.
//! Sessions live in memory only — a server restart logs every browser out,
//! which is fine for a single-user program.
//!
//! **The token.** `zdesk serve-worker` has no user to ask and nowhere to keep
//! a credential: it is started for one managed browser instance. So it mints an
//! [`WorkerToken`], hands it to its parent down a pipe (see [`crate::worker`]),
//! and takes it in the same `zdesk_session` cookie a login would have set. The
//! token is not a browser's way in: the browser logs in to the parent control
//! plane with the PIN its TUI shows, and the control plane adds the token to each
//! request it forwards, `/api/*` and the WebSocket upgrades alike. Such a gateway
//! still refuses `/api/auth/login` and `/api/auth/logout`: there is no credential
//! to check and no session to end.
//!
//! So a browser has two ways in, one per kind of gateway: a served gateway's
//! username and password, and the TUI's PIN.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

#[cfg(feature = "tui")]
use base64::Engine as _;
#[cfg(feature = "tui")]
use rand::RngExt as _;
use sha2::Digest as _;

use crate::vault::{LoginHash, LoginKey};

/// Name of the auth session cookie.
pub const COOKIE_NAME: &str = "zdesk_session";

/// Sliding session lifetime: every validated request pushes the expiry out.
const SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// The fewest characters a web login's password may have: NIST SP 800-63B's
/// minimum for a password a person chooses.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// Whether `password` is long enough to be a web login's.
fn long_enough(password: &str) -> bool {
    password.chars().count() >= MIN_PASSWORD_CHARS
}

/// Refuse a web login password shorter than [`MIN_PASSWORD_CHARS`]: checked where
/// the password is still plain, since a hash cannot tell its length.
pub fn check_password_length(password: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        long_enough(password),
        "the web login password must be at least {MIN_PASSWORD_CHARS} characters"
    );
    Ok(())
}

/// A served gateway's web login: the one username allowed to log in,
/// `[server].site_username`, and `[server].site_passwd_hash` with the key it was
/// made under, given by [`crate::config::ConfigFile::unlock`].
///
/// The username is kept as a SHA-256 digest, so it is compared at one length
/// whatever was typed. The password is checked by one HMAC under the login key, so
/// a check costs what serving a page does, and nothing the gateway holds is the
/// password or opens to it.
#[derive(Clone)]
pub struct SiteLogin {
    username: String,
    username_digest: [u8; 32],
    hash: LoginHash,
    /// `None` when the config was only checked, never unlocked: such a login lets
    /// nobody in.
    key: Option<LoginKey>,
}

// Manual Debug, so an AppConfig dump prints the username and nothing else.
impl std::fmt::Debug for SiteLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteLogin")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl SiteLogin {
    /// The login for `username`, whose password `hash` was made under `key`. A
    /// hash of the empty password is refused once the key can tell it.
    pub fn new(username: &str, hash: LoginHash, key: Option<LoginKey>) -> anyhow::Result<Self> {
        anyhow::ensure!(!username.is_empty(), "the login username is empty");
        anyhow::ensure!(
            !key.as_ref().is_some_and(|key| key.verify("", &hash)),
            "the login password is empty"
        );
        Ok(Self {
            username: username.to_owned(),
            username_digest: digest(username),
            hash,
            key,
        })
    }

    /// The login for `username` and `password` made in memory, under a login key
    /// of its own: for a gateway built without a config file.
    pub fn with_password(username: &str, password: &str) -> anyhow::Result<Self> {
        check_password_length(password)?;
        let key = LoginKey::generate();
        Self::new(username, key.hash(password), Some(key))
    }

    /// The username allowed to log in (for startup logging).
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Check a login attempt. Both halves are compared in full whatever the other
    /// said, so a wrong username and a wrong password take the same time; and a
    /// password shorter than [`MIN_PASSWORD_CHARS`], the empty one included, is
    /// never right.
    pub fn verify(&self, username: &str, password: &str) -> bool {
        let username_ok = bytes_match(&digest(username), &self.username_digest);
        let password_ok = self.key.as_ref().is_some_and(|key| key.verify(password, &self.hash));
        username_ok & password_ok & long_enough(password)
    }
}

fn digest(text: &str) -> [u8; 32] {
    sha2::Sha256::digest(text.as_bytes()).into()
}

/// How this process decides that a request may proceed at all.
///
/// One of the two, fixed at startup: a `serve` gateway can only be a
/// [`Self::Login`] and a `serve-worker` one can only be a [`Self::Token`]. That
/// is the point of an enum here rather than two optional fields — neither mode
/// carries a way in that it does not use, and `require_auth` in
/// [`crate::server`] matches on this and on nothing else.
#[derive(Clone, Debug)]
pub enum GatewayAuth {
    /// A web login: the `[server]` username and password, the `zdesk_session`
    /// cookie, and the [`AuthSessions`] behind it.
    Login(SiteLogin),
    /// One token, minted at startup and given to the control plane that started
    /// this gateway, which presents it for its logged-in browsers as the same
    /// `zdesk_session` cookie a login would have set. `/api/auth/login` and `/api/auth/logout` refuse on such a
    /// gateway: there is nothing to log in to and nothing to end.
    #[cfg(feature = "tui")]
    Token(WorkerToken),
}

impl GatewayAuth {
    /// The login credential when this is a deployed gateway. A managed gateway
    /// has no interactive login endpoint.
    pub fn login(&self) -> Option<&SiteLogin> {
        match self {
            Self::Login(login) => Some(login),
            #[cfg(feature = "tui")]
            Self::Token(_) => None,
        }
    }

    /// Validate the cookie using the only authentication scheme compiled into
    /// and selected for this process.
    pub fn authenticates(&self, sessions: &AuthSessions, presented: &str) -> bool {
        match self {
            Self::Login(_) => sessions.validate(presented),
            #[cfg(feature = "tui")]
            Self::Token(expected) => expected.matches(presented),
        }
    }
}

/// The secret a worker and its one client share: 32 random bytes,
/// base64url, minted per launch.
///
/// Deliberately not hashed under the master key the way a [`SiteLogin`]'s password
/// is. That protects a credential at rest against whoever reads where it rests,
/// and this one never rests: it is generated in memory, written once to a pipe
/// held by the process that spawned this one, and gone when this one exits.
#[cfg(feature = "tui")]
#[derive(Clone)]
pub struct WorkerToken(String);

// Manual Debug so a config dump — which carries the whole `GatewayAuth` — cannot
// print the one secret that would let anything drive the remote.
#[cfg(feature = "tui")]
impl std::fmt::Debug for WorkerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkerToken(…)")
    }
}

#[cfg(feature = "tui")]
impl WorkerToken {
    /// How many random bytes the token carries.
    const BYTES: usize = 32;

    /// Mint a fresh token from the OS random source.
    pub fn generate() -> Self {
        let mut bytes = [0u8; Self::BYTES];
        rand::rng().fill(&mut bytes);
        Self(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    /// The value to hand the client, and the only place it is readable.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether a presented value is this token, without leaking *where* it stops
    /// matching.
    ///
    /// The length is not a secret — every token this mints is the same length — so
    /// a mismatched length answers immediately. The bytes are, so they are folded
    /// into one accumulator rather than compared with an early return.
    pub fn matches(&self, presented: &str) -> bool {
        secrets_match(&self.0, presented)
    }
}

/// Whether two secrets are the same, without leaking *where* they stop matching.
///
/// A free function because the same token is checked in two places: here, by the
/// worker holding the [`WorkerToken`], and by the control plane's
/// router, which reads it out of a cookie before proxying
/// ([`crate::worker::manager`]) and never holds the token type at all. One
/// comparison, so there is no second copy to be the one that compares with `==`.
#[cfg(feature = "tui")]
pub fn secrets_match(expected: &str, presented: &str) -> bool {
    bytes_match(expected.as_bytes(), presented.as_bytes())
}

/// Whether two byte strings are the same, folding every byte into one accumulator
/// rather than returning at the first difference. A length mismatch answers at
/// once: the callers compare values whose length is not the secret.
fn bytes_match(expected: &[u8], presented: &[u8]) -> bool {
    if expected.len() != presented.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in expected.iter().zip(presented) {
        difference |= a ^ b;
    }
    difference == 0
}

/// Failed logins allowed in a row before each further one locks logins out.
const FREE_FAILURES: u32 = 5;

/// The longest lockout a run of failures earns.
const MAX_LOCKOUT: Duration = Duration::from_secs(60);

/// The one throttle on login attempts, for the whole process.
///
/// Not by client address: behind a reverse proxy every attempt comes from the
/// proxy, and a forwarded header is whatever the client wrote. So it counts failed
/// attempts in a row, whoever made them. The first [`FREE_FAILURES`] cost nothing;
/// each one after locks logins out for 1 s, then 2 s, 4 s and so on up to
/// [`MAX_LOCKOUT`], and an attempt during a lockout is refused without being
/// checked, the right password included. A successful login clears the count.
///
/// The price is that someone guessing can keep the one user locked out too, for at
/// most [`MAX_LOCKOUT`] past their last guess: for a single-user program, a short
/// wait for its user over unlimited guesses for anyone.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    state: Mutex<ThrottleState>,
}

#[derive(Debug, Default)]
struct ThrottleState {
    failures: u32,
    locked_until: Option<Instant>,
}

impl LoginThrottle {
    /// Run the check `verify` makes, unless a lockout is running at `now`: then
    /// `Err` with how long it has left. The lockout, the check and its count are
    /// one step under the lock, so a burst of attempts sent at once is counted
    /// one by one rather than all let through before the first failure lands.
    pub fn attempt(&self, now: Instant, verify: impl FnOnce() -> bool) -> Result<bool, Duration> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(until) = state.locked_until.filter(|until| *until > now) {
            return Err(until - now);
        }
        let ok = verify();
        if ok {
            *state = ThrottleState::default();
        } else {
            state.failures = state.failures.saturating_add(1);
            if let Some(over) = state.failures.checked_sub(FREE_FAILURES + 1) {
                let lockout = Duration::from_secs(1u64 << over.min(16)).min(MAX_LOCKOUT);
                state.locked_until = Some(now + lockout);
            }
        }
        Ok(ok)
    }
}

/// In-memory auth sessions: token → expiry, with a sliding TTL. The map only
/// ever holds this single user's handful of logins, so expired entries are
/// swept opportunistically on create/validate instead of by a timer.
pub struct AuthSessions {
    sessions: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
}

impl Default for AuthSessions {
    fn default() -> Self {
        Self::with_ttl(SESSION_TTL)
    }
}

impl AuthSessions {
    fn with_ttl(ttl: Duration) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Mint a session token after a successful login.
    pub fn create(&self) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        let mut sessions = self.sessions.lock().unwrap();
        let now = Instant::now();
        sessions.retain(|_, expiry| *expiry > now);
        sessions.insert(token.clone(), now + self.ttl);
        token
    }

    /// True when the token names a live session; refreshes the sliding expiry.
    pub fn validate(&self, token: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let now = Instant::now();
        match sessions.get_mut(token) {
            Some(expiry) if *expiry > now => {
                *expiry = now + self.ttl;
                true
            }
            Some(_) => {
                sessions.remove(token);
                false
            }
            None => false,
        }
    }

    /// Drop a session (logout).
    pub fn invalidate(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }
}

/// Extract this app's session token from a request's `Cookie` header.
pub fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE_NAME).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::*;

    fn login(password: &str, unlocked: bool) -> SiteLogin {
        let key = LoginKey::generate();
        SiteLogin::new("admin", key.hash(password), unlocked.then_some(key)).unwrap()
    }

    #[test]
    fn a_login_takes_its_username_and_password_and_nothing_else() {
        let login = login("hunter22", true);
        assert_eq!(login.username(), "admin");
        assert!(login.verify("admin", "hunter22"));
        assert!(!login.verify("admin", "wrong"));
        assert!(!login.verify("admin", "hunter2"), "a prefix");
        assert!(!login.verify("mallory", "hunter22"), "username must match too");
        assert!(SiteLogin::with_password("", "hunter22").is_err());
    }

    /// A config that is only checked resolves its login with no key: that login
    /// must open to nothing.
    #[test]
    fn a_login_without_its_key_never_logs_in() {
        let login = login("hunter22", false);
        assert!(!login.verify("admin", "hunter22"));
    }

    /// Five failures are free; each after locks out for twice as long, up to the
    /// cap, refusing even the right password; and a success clears the count.
    #[test]
    fn failed_logins_lock_out_longer_each_time() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..FREE_FAILURES {
            assert_eq!(throttle.attempt(start, || false), Ok(false));
        }
        assert_eq!(throttle.attempt(start, || false), Ok(false), "the sixth is checked");
        assert_eq!(throttle.attempt(start, || true), Err(Duration::from_secs(1)), "then locked");

        let later = start + Duration::from_secs(1);
        assert_eq!(throttle.attempt(later, || false), Ok(false));
        assert_eq!(throttle.attempt(later, || unreachable!()), Err(Duration::from_secs(2)));

        let mut now = later;
        for _ in 0..20 {
            now += MAX_LOCKOUT;
            assert_eq!(throttle.attempt(now, || false), Ok(false));
        }
        assert_eq!(throttle.attempt(now, || true), Err(MAX_LOCKOUT), "capped");

        now += MAX_LOCKOUT;
        assert_eq!(throttle.attempt(now, || true), Ok(true));
        for _ in 0..FREE_FAILURES {
            assert_eq!(throttle.attempt(now, || false), Ok(false), "a success clears the count");
        }
    }

    /// The empty password never logs in: a login for it is refused when it is made,
    /// and a login for another password turns it away.
    #[test]
    fn an_empty_password_never_logs_in() {
        assert!(SiteLogin::with_password("admin", "").is_err());
        let key = LoginKey::generate();
        assert!(SiteLogin::new("admin", key.hash(""), Some(key)).is_err());
        assert!(!login("hunter22", true).verify("admin", ""));
    }

    /// A password must have [`MIN_PASSWORD_CHARS`] characters, counted as
    /// characters rather than bytes, and a shorter attempt is never right.
    #[test]
    fn a_short_password_is_refused() {
        assert!(check_password_length("hunter2").is_err());
        assert!(check_password_length("hunter22").is_ok());
        assert!(check_password_length("密码密码密码密码").is_ok(), "eight characters");
        assert!(check_password_length("密码密码密码密").is_err(), "seven, in 21 bytes");
        assert!(SiteLogin::with_password("admin", "hunter2").is_err());
        let key = LoginKey::generate();
        let short = SiteLogin::new("admin", key.hash("short"), Some(key)).unwrap();
        assert!(!short.verify("admin", "short"), "a short hash made in code still lets nobody in");
    }

    #[test]
    fn a_login_is_not_in_its_debug_output() {
        let rendered = format!("{:?}", GatewayAuth::Login(login("hunter22", true)));
        assert!(rendered.contains("admin") && !rendered.contains("hunter22"), "{rendered}");
    }

    #[test]
    fn sessions_validate_refresh_and_invalidate() {
        let sessions = AuthSessions::default();
        let token = sessions.create();
        assert!(sessions.validate(&token));
        assert!(!sessions.validate("no-such-token"));
        sessions.invalidate(&token);
        assert!(!sessions.validate(&token));
    }

    #[test]
    fn expired_sessions_are_rejected() {
        let sessions = AuthSessions::with_ttl(Duration::ZERO);
        let token = sessions.create();
        assert!(!sessions.validate(&token), "a zero-TTL session is born expired");
    }

    /// A minted token matches itself and nothing else, including the near misses a
    /// hand-rolled constant-time compare could get wrong: a prefix, an extension,
    /// and the empty string.
    #[cfg(feature = "tui")]
    #[test]
    fn a_worker_token_matches_only_itself() {
        let token = WorkerToken::generate();
        assert!(token.matches(token.as_str()));
        assert!(!token.matches(""));
        assert!(!token.matches(&token.as_str()[..token.as_str().len() - 1]), "a prefix");
        assert!(!token.matches(&format!("{}x", token.as_str())), "an extension");
        assert!(!token.matches(WorkerToken::generate().as_str()), "another token");
    }

    /// Two launches never share a secret, and the encoding is URL-safe base64 so
    /// the value can sit in a header untouched.
    #[cfg(feature = "tui")]
    #[test]
    fn worker_tokens_are_fresh_and_header_safe() {
        let (a, b) = (WorkerToken::generate(), WorkerToken::generate());
        assert_ne!(a.as_str(), b.as_str());
        // 32 bytes, base64 without padding.
        assert_eq!(a.as_str().len(), 43, "{}", a.as_str());
        assert!(
            a.as_str()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{}",
            a.as_str()
        );
    }

    /// The one secret that would drive the remote must not be printable by
    /// accident: `AppConfig` derives Debug and carries this.
    #[cfg(feature = "tui")]
    #[test]
    fn a_worker_token_is_not_in_its_debug_output() {
        let token = WorkerToken::generate();
        let auth = GatewayAuth::Token(token.clone());
        for rendered in [format!("{token:?}"), format!("{auth:?}")] {
            assert!(!rendered.contains(token.as_str()), "{rendered}");
        }
        assert!(auth.login().is_none(), "a token gateway has no login");
    }

    #[test]
    fn token_is_found_among_other_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=1; {COOKIE_NAME}=tok-123; x=y")).unwrap(),
        );
        assert_eq!(token_from_headers(&headers).as_deref(), Some("tok-123"));

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("other=1"));
        assert_eq!(token_from_headers(&headers), None);
        assert_eq!(token_from_headers(&HeaderMap::new()), None);
    }
}
