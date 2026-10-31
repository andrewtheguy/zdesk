//! Who may talk to this gateway: the web login for a browser, or one minted
//! token for the client a worker was started by.
//!
//! A process has exactly one of them ([`GatewayAuth`], chosen at startup), which
//! is what keeps each mode free of the other's machinery.
//!
//! **The login.** `[server].site_username` names the one user, and
//! `[server].site_passwd_enc` holds that user's password, encrypted like a
//! target's (see [`crate::vault`]). `/api/auth/login` verifies a
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
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[cfg(feature = "tui")]
use base64::Engine as _;
#[cfg(feature = "tui")]
use rand::RngExt as _;
use sha2::Digest as _;

/// Name of the auth session cookie.
pub const COOKIE_NAME: &str = "zdesk_session";

/// Sliding session lifetime: every validated request pushes the expiry out.
const SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Argon2id's cost for checking a login: 19 MiB and two passes, OWASP's minimum.
/// Every guess at `/api/auth/login` pays it, which is what makes guessing a human's
/// password slow; a lighter cost than the vault's, which is paid once per start.
const LOGIN_ARGON2_MEMORY_KIB: u32 = 19 * 1024;
const LOGIN_ARGON2_PASSES: u32 = 2;
const LOGIN_SALT_LEN: usize = 16;

/// A served gateway's web login: the one username allowed to log in and its
/// password, `[server].site_username` and `[server].site_passwd_enc`, decrypted by
/// [`crate::config::ConfigFile::unlock`].
///
/// The username is kept as a SHA-256 digest, so it is compared at one length
/// whatever was typed. The password is kept as Argon2id of it under a salt drawn
/// at start, so the password itself is not held for the life of the process and
/// every guess costs [`LOGIN_ARGON2_MEMORY_KIB`] and its passes.
#[derive(Clone)]
pub struct SiteLogin {
    username: String,
    username_digest: [u8; 32],
    /// `None` when no password was decrypted (a config only checked, not
    /// served): such a login lets nobody in.
    password_hash: Option<[u8; 32]>,
    salt: [u8; LOGIN_SALT_LEN],
    /// One check at a time. Each takes Argon2id's memory, so a flood of guesses
    /// waits here in turn rather than allocating it once per guess at once.
    checking: Arc<Mutex<()>>,
}

// Manual Debug so an AppConfig dump never prints a hash of the password.
impl std::fmt::Debug for SiteLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteLogin")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl SiteLogin {
    /// The login for `username` and its decrypted `password`. An empty password
    /// is one never decrypted, and opens to nothing.
    pub fn new(username: &str, password: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(!username.is_empty(), "the login username is empty");
        let mut salt = [0u8; LOGIN_SALT_LEN];
        rand::fill(&mut salt[..]);
        let password_hash = (!password.is_empty()).then(|| stretch(password, &salt)).transpose()?;
        Ok(Self {
            username: username.to_owned(),
            username_digest: digest(username),
            password_hash,
            salt,
            checking: Arc::new(Mutex::new(())),
        })
    }

    /// The username allowed to log in (for startup logging).
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Check a login attempt. Both halves are compared in full whatever the other
    /// said, so a wrong username and a wrong password take the same time. Blocks
    /// for Argon2id's cost and for any check ahead of it, so call it off the
    /// async workers.
    pub fn verify(&self, username: &str, password: &str) -> bool {
        let username_ok = bytes_match(&digest(username), &self.username_digest);
        let Some(expected) = &self.password_hash else {
            return false;
        };
        let _turn = self.checking.lock().unwrap_or_else(PoisonError::into_inner);
        let password_ok = stretch(password, &self.salt).is_ok_and(|hash| bytes_match(&hash, expected));
        username_ok & password_ok
    }
}

fn digest(text: &str) -> [u8; 32] {
    sha2::Sha256::digest(text.as_bytes()).into()
}

/// Argon2id of a login password under `salt`, at the login's cost.
fn stretch(password: &str, salt: &[u8; LOGIN_SALT_LEN]) -> anyhow::Result<[u8; 32]> {
    let params = argon2::Params::new(LOGIN_ARGON2_MEMORY_KIB, LOGIN_ARGON2_PASSES, 1, Some(32))
        .map_err(|error| anyhow::anyhow!("invalid Argon2 parameters: {error}"))?;
    let mut hash = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut hash)
        .map_err(|error| anyhow::anyhow!("Argon2id failed: {error}"))?;
    Ok(hash)
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
/// Deliberately neither encrypted nor stretched the way a [`SiteLogin`]'s password
/// is. Encryption protects a credential at rest against whoever reads where it
/// rests, and this one never rests: it is generated in memory, written once to a
/// pipe held by the process that spawned this one, and gone when this one exits.
/// And there is nobody to be slow for: Argon2id's cost exists to make guessing a
/// *human's* password expensive, and 32 random bytes are not guessable at any cost.
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

    #[test]
    fn a_login_takes_its_username_and_password_and_nothing_else() {
        let login = SiteLogin::new("admin", "hunter2").unwrap();
        assert_eq!(login.username(), "admin");
        assert!(login.verify("admin", "hunter2"));
        assert!(!login.verify("admin", "wrong"));
        assert!(!login.verify("admin", "hunter"), "a prefix");
        assert!(!login.verify("mallory", "hunter2"), "username must match too");
        assert!(SiteLogin::new("", "hunter2").is_err());
    }

    /// A config that is only checked resolves its login with the password never
    /// decrypted: that login must open to nothing, the empty password included.
    #[test]
    fn an_empty_password_never_logs_in() {
        let login = SiteLogin::new("admin", "").unwrap();
        assert!(!login.verify("admin", ""));
    }

    #[test]
    fn a_login_is_not_in_its_debug_output() {
        let login = SiteLogin::new("admin", "hunter2").unwrap();
        let rendered = format!("{:?}", GatewayAuth::Login(login));
        assert!(rendered.contains("admin") && !rendered.contains("hunter2"), "{rendered}");
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
