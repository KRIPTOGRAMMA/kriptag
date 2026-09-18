// Extended activity tracking on X11 via the MIT-SCREEN-SAVER extension.
//
// The third system idle source, after wayland_idle (ext-idle-notify-v1) and
// windows_idle (GetLastInputInfo), and the same shape: start(tracker) -> bool
// spawns a ticker that keeps tracker.last_input fresh while the user is active
// anywhere in the session. Without a system source last_input only moves when
// our own window sees input, which is the false "you were away for N minutes"
// of v0.10.29.
//
// X11 hands over a ready quantity — ms_since_user_input — so unlike Windows
// there is no tick counter to unwrap: the server reports elapsed time, not an
// absolute count.
//
// THE EXTENSION IS OPTIONAL, AND ITS ABSENCE IS NOT AN ERROR. Verified live on
// Xwayland: MIT-SCREEN-SAVER is missing there, XScreenSaverQueryExtension says
// so, and yet the query still "succeeds" — filling the struct with zeroes. Read
// without checking, that is a permanent idle time of 0, i.e. "the user is always
// active", and the state machine would never declare idleness at all. A lie
// mirroring the 360 minutes rather than repeating it. Hence query_version
// before anything else, and false when it is not there.

use std::sync::Arc;

use super::activity::ActivityTracker;

use super::{IDLE_CUTOFF_MS, IDLE_TICK_SECS};

// Whether the reported idle time means the user is still around.
//
// Pure, so the boundary is testable without an X server: the ticker cannot be
// asked to produce a given idle time, this can.
pub fn is_active(idle_ms: u32) -> bool {
    idle_ms < IDLE_CUTOFF_MS
}

#[cfg(target_os = "linux")]
pub fn start(tracker: Arc<ActivityTracker>) -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::screensaver::ConnectionExt as _;

    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(v) => v,
        Err(_) => return false, // no X server reachable
    };

    // The extension is optional. Asking for its version is the documented way to
    // find out whether it is there; querying without this check yields zeroes
    // that read as "always active".
    //
    // Verified live against Xwayland: the absence arrives as Err(UnsupportedExtension)
    // from the request itself, not as an error reply, so the Err arm is the one
    // that actually fires — both are handled because a server may also refuse
    // the reply.
    if conn
        .screensaver_query_version(1, 1)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_none()
    {
        return false;
    }

    let root = match conn.setup().roots.get(screen_num) {
        Some(screen) => screen.root,
        None => return false,
    };

    // One probe before committing: if the query cannot answer now it will not
    // answer later, and a ticker that learns nothing would leave the application
    // believing it has a system source when it has none.
    if probe_idle_millis(&conn, root).is_none() {
        return false;
    }

    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(IDLE_TICK_SECS));
        // A failure mid-run is not a reason to invent input: skip the tick and
        // let the state machine decide from the last honest value.
        if let Some(idle_ms) = probe_idle_millis(&conn, root) {
            if is_active(idle_ms) {
                tracker.record_input();
            }
        }
    });

    true
}

// One query. Every X call lives here; nothing else in the module talks to the
// server.
#[cfg(target_os = "linux")]
fn probe_idle_millis<C>(conn: &C, root: u32) -> Option<u32>
where
    C: x11rb::connection::Connection,
{
    use x11rb::protocol::screensaver::ConnectionExt as _;

    conn.screensaver_query_info(root)
        .ok()?
        .reply()
        .ok()
        .map(|info| info.ms_since_user_input)
}

// X11 is a Linux/Unix display protocol here; elsewhere the signature is kept so
// callers need no cfg of their own.
#[cfg(not(target_os = "linux"))]
pub fn start(_tracker: Arc<ActivityTracker>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // The cutoff is what the ticker compares against, so a value on the wrong
    // side of it would either never refresh last_input or never stop.
    #[test]
    fn the_cutoff_sits_below_the_applications_threshold() {
        assert!(
            IDLE_CUTOFF_MS < 300_000,
            "порог источника ({IDLE_CUTOFF_MS} мс) должен быть заметно меньше \
             порога приложения (300 с), иначе приложение объявит простой раньше, \
             чем источник перестанет обновлять last_input"
        );
        assert!(
            (IDLE_TICK_SECS * 1000) < IDLE_CUTOFF_MS as u64,
            "опрос ({IDLE_TICK_SECS} с) должен быть чаще порога источника, иначе \
             активность между опросами потеряется"
        );
    }

    #[test]
    fn activity_is_judged_by_the_cutoff() {
        assert!(is_active(0), "ввод только что");
        assert!(is_active(IDLE_CUTOFF_MS - 1));
        assert!(!is_active(IDLE_CUTOFF_MS), "порог включающий, как и в step_idle");
        assert!(!is_active(6 * 60 * 60 * 1000), "шесть часов — точно не активность");
    }

}
