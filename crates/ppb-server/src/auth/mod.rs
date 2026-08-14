//! Auth domain: JWT, sessions, Phira login, Root, reauth, GitHub bind.

pub mod consent;
pub mod gateway;
pub mod github;
pub mod jwt;
pub mod phira;
pub mod reauth;
pub mod root;
pub mod routes;
pub mod session;
pub mod types;

/// HttpOnly access JWT cookie.
pub const ACCESS_COOKIE: &str = "ppb_access";
/// HttpOnly session refresh cookie.
pub const REFRESH_COOKIE: &str = "ppb_refresh";
