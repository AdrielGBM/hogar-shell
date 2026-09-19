//! Whether this process may reach the user's live session: the D-Bus buses, the compositor's IPC socket and the audio server; until [`install`] runs, every door here answers as if no daemon is present, so a test or preview takes the same degraded path as a machine missing one.

use std::sync::OnceLock;

use zbus::blocking::connection::Builder;

static INSTALLED: OnceLock<()> = OnceLock::new();

/// Lets this process reach the user's session.
pub fn install() {
    let _ = INSTALLED.set(());
}

/// Whether [`install`] has run. A connection to anything in the user's session is opened only when this answers `true`.
pub fn installed() -> bool {
    INSTALLED.get().is_some()
}

/// A connection builder for the session bus, or `None` when this process is sealed from it or there is no bus to reach. The only place a session-bus connection starts.
pub fn session_bus() -> Option<Builder<'static>> {
    installed().then(Builder::session)?.ok()
}

/// A connection builder for the system bus, or `None` when this process is sealed from it or there is no bus to reach. The only place a system-bus connection starts.
pub fn system_bus() -> Option<Builder<'static>> {
    installed().then(Builder::system)?.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_that_never_installed_reaches_no_bus() {
        assert!(!installed(), "nothing under test opts in");
        assert!(session_bus().is_none());
        assert!(system_bus().is_none());
    }

    /// The seal only holds if nothing opens a bus connection for itself, so the constructors that do are named here and nowhere else.
    #[test]
    fn only_this_module_opens_a_bus_connection() {
        for needle in [
            "Builder::session",
            "Builder::system",
            "Builder::address",
            "Connection::session",
            "Connection::system",
        ] {
            let offenders = crate::deps::tests::sources_containing(needle, &["util/src/live.rs"]);
            assert!(
                offenders.is_empty(),
                "these open a bus connection without `util::live` (`{needle}`): {offenders:#?}"
            );
        }
    }
}
