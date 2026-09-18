pub mod activity;

// Shared by every system idle source (wayland_idle, windows_idle, x11_idle), so
// that "extended mode" means the same precision on every OS rather than three
// accidental cadences.
//
// TICK_SECS is an order of magnitude below the application's idle threshold
// (idle_threshold_secs, 300 s by default), so polling this rarely costs nothing
// and still cannot misplace the transition. IDLE_CUTOFF_MS is the point past
// which a source stops refreshing last_input and lets the application's own
// state machine declare the user idle: the source only answers "is there input
// right now", the threshold is the application's business.
pub const IDLE_TICK_SECS: u64 = 15;
pub const IDLE_CUTOFF_MS: u32 = 30_000;
// Wayland exists only on Linux; the module's dependencies are declared for that
// target alone, so compiling it anywhere else fails on the missing crates.
#[cfg(target_os = "linux")]
pub mod wayland_idle;
// X11 lives alongside Wayland on Linux: a session is one or the other, decided
// at run time, so both modules are compiled and the probe picks.
#[cfg(target_os = "linux")]
pub mod x11_idle;
// The Windows counterpart. Unlike wayland_idle this one is NOT gated: its pure
// arithmetic (the tick-counter wrap) is tested on every platform, and the FFI
// inside is what carries the cfg. Off Windows the helpers are reached only from
// those tests, hence the allow — the alternative would be cfg-ing the tests off
// the platforms where they are the only thing proving the arithmetic.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_idle;
pub mod window;
pub mod domain;
