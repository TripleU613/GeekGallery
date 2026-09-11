//! Mint a session cookie for a given user id, for local testing only.
//!
//! Exists because the admin-only routes cannot be exercised without a valid
//! session, and the only other way to get one is to complete a Google login in a
//! browser. This uses the app's own key derivation and cookie name, so what it
//! produces is exactly what the server would have issued -- which is the point: a
//! test that forges its own auth differently proves nothing about the real path.
//!
//!   SESSION_SECRET=... cargo run --example mint_test_session --features ssr -- <user-id>
//!
//! Not a route, not compiled into the binary, and useless without SESSION_SECRET.
fn main() {
    let id = std::env::args()
        .nth(1)
        .expect("usage: mint_test_session <user-id>");
    match geekgallery::auth::debug_session_cookie(&id) {
        Some(value) => println!("session={value}"),
        None => eprintln!("SESSION_SECRET is unset or too short; nothing minted"),
    }
}
