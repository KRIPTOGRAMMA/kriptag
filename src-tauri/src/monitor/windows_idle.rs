// Extended activity tracking on Windows via GetLastInputInfo.
//
// The counterpart of monitor::wayland_idle, and the shape is the same:
// start(tracker) spawns a ticker that keeps tracker.last_input fresh while the
// user is active ANYWHERE in the system, so that working in another
// application is no longer indistinguishable from having left the desk. Without
// it Windows stays on the basic mode, where last_input only moves when our own
// window sees input — the false "you were away for 360 minutes" of v0.10.29.
//
// Where Wayland reports transitions (Idled/Resumed) and needs a ticker to
// reconstruct the activity between them, Win32 reports a quantity: the tick
// count at which input last reached ANY window of the session. So there is no
// event loop here, only polling — and the whole subtlety sits in the
// arithmetic, see idle_millis.

use std::sync::Arc;

use super::activity::ActivityTracker;

// Off Windows these are reached only from the tests below, which are the sole
// proof of the wrap arithmetic and so must run everywhere.
#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
use super::{IDLE_CUTOFF_MS, IDLE_TICK_SECS};

// Milliseconds since the last input, from the two raw Win32 values.
//
// Both are 32-bit millisecond tick counts, and that is the whole point of this
// function: GetTickCount wraps around every ~49.7 days, after which `now` is a
// small number and `last_input` a large one. A plain subtraction then yields a
// huge positive value (or panics in debug) and the application would report an
// absence of weeks — the same class of lie as v0.10.29, arrived at from the
// other side. wrapping_sub gives the true elapsed time across the wrap, because
// the difference of two wrapped counters is itself correct modulo 2^32.
//
// Extracted from the FFI so the wrap can be covered by a test: the syscall
// cannot be made to return an arbitrary tick count, but this can.
pub fn idle_millis(now_ticks: u32, last_input_ticks: u32) -> u32 {
    now_ticks.wrapping_sub(last_input_ticks)
}

// Tries to start extended tracking. On Windows the API is always present (it is
// user32, not an optional protocol), so this reports whether the call actually
// answers: a session with no input desktop — a service, a locked-down context —
// returns FALSE, and then we must not pretend to know about presence.
#[cfg(target_os = "windows")]
pub fn start(tracker: Arc<ActivityTracker>) -> bool {
    // One probe before committing: if the call fails here it will keep failing,
    // and a ticker that never learns anything would leave the application
    // believing it has a system source when it has none.
    if probe_idle_millis().is_none() {
        return false;
    }

    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(IDLE_TICK_SECS));
        // A failure mid-run is not a reason to invent input: skip the tick and
        // let the state machine decide from the last honest value.
        if let Some(idle_ms) = probe_idle_millis() {
            if idle_ms < IDLE_CUTOFF_MS {
                tracker.record_input();
            }
        }
    });

    true
}

// The syscall itself: everything unsafe lives here and nothing else in the
// module touches FFI.
#[cfg(target_os = "windows")]
fn probe_idle_millis() -> Option<u32> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO {
        // The API validates this field and fails outright when it disagrees with
        // the struct it was compiled against.
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };

    // SAFETY: a well-formed LASTINPUTINFO with cbSize set, as the API requires;
    // the pointer is to a live local that outlives the call.
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if ok == 0 {
        return None;
    }

    // SAFETY: no arguments, no pointers — it just reads the system tick counter.
    let now = unsafe { GetTickCount() };
    Some(idle_millis(now, info.dwTime))
}

// Elsewhere this source does not exist; the signature is kept so that callers
// need no cfg of their own.
#[cfg(not(target_os = "windows"))]
pub fn start(_tracker: Arc<ActivityTracker>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // GetTickCount wraps every ~49.7 days. A plain `now - last` would then read
    // as an absence of weeks, which is exactly the bug v0.10.29 fixed from the
    // other end — so it must be tested, not assumed.
    #[test]
    fn the_tick_counter_wrapping_around_does_not_invent_an_absence() {
        // 5 s before the wrap, 3 s after it: 8 s of real idleness.
        let last_input = u32::MAX - 5_000 + 1;
        let now = 3_000u32;
        assert_eq!(
            idle_millis(now, last_input),
            8_000,
            "через переполнение счётчика (каждые ~49.7 суток) обычное вычитание \
             дало бы ~4 млрд мс — «пользователь отсутствовал 49 суток». Именно \
             такую ложь чинила v0.10.29, здесь она приходит с другой стороны."
        );
    }

    #[test]
    fn the_ordinary_case_is_a_plain_difference() {
        assert_eq!(idle_millis(100_000, 40_000), 60_000);
        assert_eq!(idle_millis(40_000, 40_000), 0, "ввод только что — нулевой простой");
    }

    // The cutoff is what the ticker compares against, so a value on the wrong
    // side of it would either never refresh last_input or never stop.
    #[test]
    fn the_cutoff_sits_below_the_applications_threshold() {
        // idle_threshold_secs defaults to 300 s; this source must react far sooner.
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
}
