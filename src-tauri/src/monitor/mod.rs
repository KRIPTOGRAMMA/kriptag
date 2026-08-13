pub mod activity;
// Wayland exists only on Linux; the module's dependencies are declared for that
// target alone, so compiling it anywhere else fails on the missing crates.
#[cfg(target_os = "linux")]
pub mod wayland_idle;
pub mod window;
pub mod domain;
