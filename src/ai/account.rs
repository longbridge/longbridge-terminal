//! Who the chat is signed in as.
//!
//! The Settings view shows this above the preferences, because "which account am
//! I asking about my portfolio?" is the one thing a reader has no other way to
//! check without leaving the chat.
//!
//! Everything here is either read from the token on disk (instant) or one cheap
//! call. Deliberately not the account name and number: those come from a
//! statement download in `longbridge auth status`, which is far too much work for
//! a settings header.

/// A snapshot of the session, for display.
#[derive(Debug, Default, Clone)]
pub struct Session {
    /// `valid`, `refresh_pending`, `expired`, `not_found`, `decrypt_failed`.
    pub status: &'static str,
    /// Data centre the token belongs to: `us` or `ap`.
    pub dc_region: Option<&'static str>,
    /// Access point the CLI is talking to (`.com` global, `.cn` mainland).
    pub access_point: String,
    /// When the token was last written, i.e. when the reader signed in.
    pub logged_in_at: Option<u64>,
    /// When the access token expires.
    pub expires_at: Option<u64>,
    /// Longbridge member id, once fetched.
    pub member_id: Option<String>,
}

impl Session {
    /// Whether a token is present and usable (or refreshable).
    pub fn signed_in(&self) -> bool {
        matches!(self.status, "valid" | "refresh_pending" | "present")
    }
}

/// Read the session from disk. No network.
pub fn local() -> Session {
    let token = crate::cli::auth::read_token_state().ok();
    Session {
        status: token.as_ref().map_or("not_found", |t| t.status),
        dc_region: token.as_ref().and_then(|t| t.dc_region),
        access_point: crate::region::http_url().to_string(),
        logged_in_at: token.as_ref().and_then(|t| t.logged_in_at),
        expires_at: token.as_ref().and_then(|t| t.access_token_exp),
        member_id: None,
    }
}

/// Fetch the member id. One call, and a failure just leaves the row out.
pub async fn member_id() -> Option<String> {
    crate::openapi::quote()
        .member_id()
        .await
        .ok()
        .map(|id| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header is read from disk, so it must render on a machine with no token
    /// at all rather than failing.
    #[test]
    fn a_session_reads_without_a_token() {
        let session = local();
        assert!(
            !session.access_point.is_empty(),
            "the access point is known"
        );
        // Whatever the machine's state, the status is one of the known ones.
        assert!(
            [
                "valid",
                "refresh_pending",
                "expired",
                "present",
                "not_found",
                "decrypt_failed"
            ]
            .contains(&session.status),
            "unexpected status {:?}",
            session.status
        );
    }

    #[test]
    fn signed_in_covers_a_refreshable_token() {
        for (status, want) in [
            ("valid", true),
            ("refresh_pending", true),
            ("present", true),
            ("expired", false),
            ("not_found", false),
            ("decrypt_failed", false),
        ] {
            let session = Session {
                status,
                ..Session::default()
            };
            assert_eq!(session.signed_in(), want, "for {status}");
        }
    }
}
