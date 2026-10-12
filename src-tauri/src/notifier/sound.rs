// Which sound a notification asks the system to play.
//
// Not a file path, and that is a platform limit rather than a choice. Every
// backend takes a NAME, not a file: Windows accepts exactly five
// (Default/IM/Mail/Reminder/SMS, see tauri-winrt-notification's FromStr), macOS
// takes a system sound name, and Linux takes a themed name from the
// freedesktop sound-naming spec. A bundled .ogg could not be handed to any of
// them through the notification API, so shipping sound files would grow the
// installer for something only one platform could play.
//
// The names also do not agree across platforms — Windows wants "Reminder" where
// Linux wants "alarm-clock-elapsed" — so what gets stored is OUR key, translated
// per platform at send time. Storing a platform name directly would mean a
// setting that stops working when the user moves machines.

/// A sound the user can pick in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySound {
    /// Whatever the system plays by default — the behaviour before this existed.
    System,
    /// No sound at all.
    Silent,
    Bell,
    Alarm,
    Message,
}

impl NotifySound {
    /// The key as stored in settings. Unknown values read as System: a setting
    /// written by a newer version, or by hand, must not silence notifications.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "silent" => NotifySound::Silent,
            "bell" => NotifySound::Bell,
            "alarm" => NotifySound::Alarm,
            "message" => NotifySound::Message,
            _ => NotifySound::System,
        }
    }

    /// The platform's own name for this sound, or None to let the system decide.
    ///
    /// Silent returns None as well: suppressing sound needs a different mechanism
    /// per platform (a hint on Linux, an empty sound on Windows) and is handled
    /// by the caller, not by naming a silent sound that does not exist.
    pub fn platform_name(self) -> Option<&'static str> {
        match self {
            NotifySound::System | NotifySound::Silent => None,

            // freedesktop sound-naming spec; verified present in
            // /usr/share/sounds/freedesktop/stereo.
            #[cfg(target_os = "linux")]
            NotifySound::Bell => Some("bell"),
            #[cfg(target_os = "linux")]
            NotifySound::Alarm => Some("alarm-clock-elapsed"),
            #[cfg(target_os = "linux")]
            NotifySound::Message => Some("message-new-instant"),

            // The five names tauri-winrt-notification parses; anything else
            // would be rejected and fall back to silence.
            #[cfg(target_os = "windows")]
            NotifySound::Bell => Some("Default"),
            #[cfg(target_os = "windows")]
            NotifySound::Alarm => Some("Reminder"),
            #[cfg(target_os = "windows")]
            NotifySound::Message => Some("IM"),

            #[cfg(target_os = "macos")]
            NotifySound::Bell => Some("Glass"),
            #[cfg(target_os = "macos")]
            NotifySound::Alarm => Some("Sosumi"),
            #[cfg(target_os = "macos")]
            NotifySound::Message => Some("Ping"),

            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            _ => None,
        }
    }

    /// Whether the user asked for no sound at all.
    pub fn is_silent(self) -> bool {
        self == NotifySound::Silent
    }
}

/// Plays the given sound once, so the user can hear what they are picking.
///
/// Shows a real notification rather than playing a file: the sound is chosen BY
/// the notification system, and anything else would preview something the user
/// will never actually hear. The key comes from the caller, not from the stored
/// setting — otherwise nothing could be auditioned before saving.
#[tauri::command]
pub fn preview_notify_sound(app: tauri::AppHandle, sound: String) {
    use tauri_plugin_notification::NotificationExt;

    let chosen = NotifySound::from_setting(&sound);
    let mut builder = app
        .notification()
        .builder()
        .title("Kriptag")
        .body("Проверка звука");
    if let Some(name) = chosen.platform_name() {
        builder = builder.sound(name);
    }
    let _ = builder.show();
}

#[cfg(test)]
mod tests {
    use super::*;

    // The stored value is OUR key, never a platform name: a settings file moved
    // between machines has to keep meaning the same thing.
    #[test]
    fn every_key_round_trips() {
        for (key, expected) in [
            ("", NotifySound::System),
            ("system", NotifySound::System),
            ("silent", NotifySound::Silent),
            ("bell", NotifySound::Bell),
            ("alarm", NotifySound::Alarm),
            ("message", NotifySound::Message),
        ] {
            assert_eq!(NotifySound::from_setting(key), expected, "ключ {key}");
        }
    }

    // An unknown value must not silence notifications: that is the one failure
    // the user would not notice until something important went unannounced.
    #[test]
    fn an_unknown_value_falls_back_to_the_system_sound() {
        for junk in ["мусор", "Reminder", "bell.oga", "BELL", "null"] {
            let s = NotifySound::from_setting(junk);
            assert_eq!(s, NotifySound::System, "«{junk}» должно читаться как системный звук");
            assert!(!s.is_silent(), "«{junk}» не должно выключать звук");
        }
    }

    #[test]
    fn system_and_silent_name_no_sound() {
        assert_eq!(NotifySound::System.platform_name(), None);
        assert_eq!(NotifySound::Silent.platform_name(), None);
        assert!(NotifySound::Silent.is_silent());
        assert!(!NotifySound::System.is_silent());
    }

    // Every pickable sound must resolve to a name on the platform being built,
    // or the option would appear in the settings and do nothing.
    #[test]
    fn every_pickable_sound_has_a_platform_name() {
        for s in [NotifySound::Bell, NotifySound::Alarm, NotifySound::Message] {
            assert!(
                s.platform_name().is_some(),
                "{s:?} не имеет имени на этой платформе — пункт в настройках был бы пустышкой"
            );
        }
    }

    // On Windows only five names parse; a typo there is silently dropped by
    // tauri-winrt-notification, so the mapping is pinned.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_names_are_from_the_accepted_five() {
        const ACCEPTED: [&str; 5] = ["Default", "IM", "Mail", "Reminder", "SMS"];
        for s in [NotifySound::Bell, NotifySound::Alarm, NotifySound::Message] {
            let name = s.platform_name().unwrap();
            assert!(ACCEPTED.contains(&name), "{name} не входит в пятёрку Windows");
        }
    }
}
