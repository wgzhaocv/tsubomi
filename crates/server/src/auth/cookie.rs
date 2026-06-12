use crate::auth::oauth_state;
use crate::auth::session::SESSION_TTL_SECS;
use axum_extra::extract::cookie::{Cookie, SameSite};
use time::Duration;

pub const SESSION_COOKIE: &str = "tsubomi_session";
pub const OAUTH_STATE_COOKIE: &str = "tsubomi_oauth_state";

pub fn build_session(value: String, secure: bool) -> Cookie<'static> {
    build(
        SESSION_COOKIE,
        value,
        secure,
        Duration::seconds(SESSION_TTL_SECS),
    )
}

pub fn build_session_clear(secure: bool) -> Cookie<'static> {
    build(SESSION_COOKIE, String::new(), secure, Duration::ZERO)
}

pub fn build_oauth_state(value: String, secure: bool) -> Cookie<'static> {
    build(
        OAUTH_STATE_COOKIE,
        value,
        secure,
        Duration::seconds(oauth_state::TTL_SECS),
    )
}

pub fn build_oauth_state_clear(secure: bool) -> Cookie<'static> {
    build(OAUTH_STATE_COOKIE, String::new(), secure, Duration::ZERO)
}

fn build(name: &'static str, value: String, secure: bool, max_age: Duration) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(max_age)
        .build()
}
