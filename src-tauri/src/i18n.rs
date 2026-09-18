// Backend localization.
//
// The scheme matches the frontend's (src/lib/i18n.ts): **the key is the Russian
// text**. The reasons hold here too: an untranslated string degrades into the
// readable Russian original rather than into "notify.deadline_now", and diffs
// stay readable by eye. Keeping two different schemes in one application would
// be worse than reusing the one that has proved itself.
//
// What is localized: only what the user actually sees outside the window —
// notifications, the tray menu, the waybar status line. Prompts to the model
// (SYSTEM_* in commands/ai.rs and planner.rs) are NOT touched: they are
// instructions for an LLM, not interface; translating them changes answer
// quality and requires verification against a live model.
//
// The language is read from the settings on every call rather than cached in a
// static: notifications are sent from background loops that live for the whole
// run, and after switching the language in Settings the very next push must
// arrive in the new one — without a restart.

use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

// An empty setting means "the user has not chosen", in which case, as on the
// frontend, anything non-Russian counts as English. The OS locale is read here
// from LANG/LC_ALL: navigator.language is not available to the backend.
pub fn lang_from_setting(saved: &str) -> Lang {
    match saved.trim() {
        "ru" => Lang::Ru,
        "en" => Lang::En,
        _ => detect_from_env(),
    }
}

fn detect_from_env() -> Lang {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if raw.to_lowercase().starts_with("ru") { Lang::Ru } else { Lang::En }
}

// The current interface language from the DB. A read error is no reason to fail
// inside a background loop: the language falls back to the environment and the
// notification is still delivered.
pub async fn current_lang(pool: &SqlitePool) -> Lang {
    let saved = crate::commands::settings::get_setting(pool, "language")
        .await
        .unwrap_or_default();
    lang_from_setting(&saved)
}

// The dictionary: Russian original -> English translation. A missing key is not
// an error — the key itself (the Russian text) is returned.
fn en(key: &str) -> Option<&'static str> {
    Some(match key {
        // --- Notifications: deadlines ---
        "Дедлайн наступил!" => "Deadline reached!",
        // Buttons on notifications (Linux only, see notifier/actions.rs)
        "Выполнено" => "Done",
        "Отложить на час" => "Snooze for an hour",
        // …on a note reminder: "Done" drops the reminder rather than completing
        // anything, hence a different word from the deadline's "Выполнено".
        "Готово" => "Done",
        "Напоминание о заметке" => "Note reminder",
        // …on pomodoro
        "Пропустить" => "Skip",
        "Ещё 5 минут" => "Five more minutes",
        "Остановить" => "Stop",
        "Дедлайн через {n} ч" => "Deadline in {n} h",
        "Дедлайн через {n} мин" => "Deadline in {n} min",
        // --- Notifications: time blocks ---
        "Начался блок (до {time})" => "Block started (until {time})",
        // --- Notifications: pomodoro ---
        "Помодоро запущено: {n} минут работы" => "Pomodoro started: {n} minutes of work",
        "Перерыв {n} минут — отдохни" => "Break for {n} minutes — take a rest",
        "Перерыв окончен: {n} минут работы" => "Break over: {n} minutes of work",
        // --- Notifications: digest, goals, limits ---
        "Утренняя сводка" => "Morning digest",
        "{cat}: {mins} мин из {limit} сегодня" => "{cat}: {mins} min of {limit} today",
        "Идёт таймер задачи «{task}», а вы в {app}." => "The timer for \"{task}\" is running, but you are in {app}.",
        "Скоро дедлайн задачи «{task}», а вы в {app}." => "\"{task}\" is due soon, but you are in {app}.",
        "Задача «{task}» в работе, а вы в {app}." => "\"{task}\" is in progress, but you are in {app}.",
        // --- Notifications: activity ---
        "Вы отсутствовали {n} мин. Продолжим задачу «{task}» или сделаем перерыв?" =>
            "You were away for {n} min. Continue the task “{task}” or take a break?",
        "Вы отсутствовали {n} мин. Ближайшая задача: {task}" =>
            "You were away for {n} min. Next up: {task}",
        "Вы отсутствовали {n} мин. С возвращением!" => "You were away for {n} min. Welcome back!",
        // --- Tray menu ---
        "Показать" => "Show",
        "Быстрая задача" => "Quick task",
        "Быстрая заметка" => "Quick note",
        "Пауза уведомлений" => "Pause notifications",
        "Выкл" => "Off",
        "30 минут" => "30 minutes",
        "1 час" => "1 hour",
        "До конца дня" => "Until end of day",
        "Бессрочно" => "Indefinitely",
        "Выход" => "Quit",
        "Режим" => "Mode",
        "Открыть" => "Open",
        "2 часа" => "2 hours",
        "Помодоро: пауза/продолжить" => "Pomodoro: pause/resume",
        "Помодоро: пропустить фазу" => "Pomodoro: skip phase",
        "{base} — осталось {n} мин" => "{base} — {n} min left",
        // --- Status line (waybar) ---
        "БД не найдена" => "Database not found",
        "пауза" => "paused",
        "до {time}" => "until {time}",
        "▶ {task} · {n} мин" => "▶ {task} · {n} min",
        "▶ {task} до {time}" => "▶ {task} until {time}",
        "Трекинг: {task} ({n} мин)" => "Tracking: {task} ({n} min)",
        "Помодоро: {phase}" => "Pomodoro: {phase}",
        "работа" => "work",
        "перерыв" => "break",
        "на паузе" => "paused",
        "Идёт: {task} (до {time})" => "Now: {task} (until {time})",
        "Идёт рутина: {task} (до {time})" => "Routine now: {task} (until {time})",
        "Далее: {task} в {time}" => "Next: {task} at {time}",
        "Далее рутина: {task} в {time}" => "Next routine: {task} at {time}",
        "В работе: {task}" => "In progress: {task}",
        "Задач на сегодня: {n}" => "Tasks due today: {n}",
        " (просрочено: {n})" => " (overdue: {n})",
        "Режим: {mode}" => "Mode: {mode}",
        // --- Context for the AI ---
        // This is not interface but data that goes into the prompt. The context's
        // language must match the prompt's: an English instruction on top of
        // Russian context does not work — the model answers in the data's language.
        "{date}: {n} мин" => "{date}: {n} min",
        "нет данных" => "no data",
        "Активные минуты по дням: {mins}. Выполнено задач за последние дни: {done}. Топ-категория выполненных задач: {cat}." =>
            "Active minutes per day: {mins}. Tasks completed in recent days: {done}. Top category of completed tasks: {cat}.",
        "Период: {label}. Выполнено задач: {done}{titles}. Активное время: {mins} мин. Просрочено сейчас: {overdue}." =>
            "Period: {label}. Tasks completed: {done}{titles}. Active time: {mins} min. Currently overdue: {overdue}.",
        "последние сутки" => "the last 24 hours",
        "последняя неделя" => "the last week",
        "последний месяц" => "the last month",
        "{done}/{total} задач" => "{done}/{total} tasks",
        "{done}/{total} мин" => "{done}/{total} min",
        " Цели проектов: {goals}." => " Project goals: {goals}.",
        "{app} ({n} мин)" => "{app} ({n} min)",
        " Топ приложений: {apps}." => " Top apps: {apps}.",
        "Сейчас {time}." => "It is now {time}.",
        "Идёт блок «{task}» до {time}." => "Block “{task}” is running until {time}.",
        "Следующий блок: {time} «{task}»." => "Next block: {time} “{task}”.",
        "Просрочено: {tasks}." => "Overdue: {tasks}.",
        // Task dependencies
        "Сначала выполните: {tasks}." => "Complete these first: {tasks}.",
        "Задача разблокирована" => "Task unblocked",
        "Можно взяться: {task}." => "Ready to start: {task}.",
        "{task} (приоритет {prio})" => "{task} (priority {prio})",
        "Активных задач нет." => "There are no active tasks.",
        "Важные задачи: {tasks}." => "Important tasks: {tasks}.",
        "Уведомления: выключены" => "Notifications: off",
        "Уведомления: пауза до {time}" => "Notifications: paused until {time}",
        _ => return None,
    })
}

/// Translates a string. The key is the Russian original; a missing translation
/// returns the key rather than an empty string.
pub fn tr(key: &str, lang: Lang) -> String {
    match lang {
        Lang::Ru => key.to_string(),
        Lang::En => en(key).unwrap_or(key).to_string(),
    }
}

/// Translation with `{name}` substitution.
pub fn tr_args(key: &str, lang: Lang, args: &[(&str, String)]) -> String {
    let mut out = tr(key, lang);
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_returns_key_unchanged() {
        assert_eq!(tr("Дедлайн наступил!", Lang::Ru), "Дедлайн наступил!");
        // even when the key is absent from the dictionary
        assert_eq!(tr("Строки нет в словаре", Lang::Ru), "Строки нет в словаре");
    }

    #[test]
    fn english_uses_dictionary() {
        assert_eq!(tr("Дедлайн наступил!", Lang::En), "Deadline reached!");
        assert_eq!(tr("Выход", Lang::En), "Quit");
    }

    // The central property of the "key = Russian text" scheme: an unfinished
    // translation degrades into a readable string rather than into emptiness or a
    // key name.
    #[test]
    fn missing_translation_falls_back_to_russian() {
        let missing = "Строка, которой точно нет 12345";
        assert_eq!(tr(missing, Lang::En), missing);
        assert!(!tr(missing, Lang::En).is_empty());
    }

    #[test]
    fn args_substituted_in_both_languages() {
        let args = [("n", "5".to_string())];
        assert_eq!(tr_args("Дедлайн через {n} мин", Lang::Ru, &args), "Дедлайн через 5 мин");
        assert_eq!(tr_args("Дедлайн через {n} мин", Lang::En, &args), "Deadline in 5 min");
    }

    #[test]
    fn unused_arg_is_harmless_missing_stays_literal() {
        assert_eq!(tr_args("Выход", Lang::Ru, &[("x", "1".into())]), "Выход");
        assert_eq!(tr_args("Значение {y}", Lang::Ru, &[]), "Значение {y}");
    }

    #[test]
    fn setting_wins_over_environment() {
        assert_eq!(lang_from_setting("ru"), Lang::Ru);
        assert_eq!(lang_from_setting("en"), Lang::En);
        assert_eq!(lang_from_setting("  ru  "), Lang::Ru);
    }

    // A translation's placeholders must match the original: a diverged {n} would
    // mean substituting into nothing and showing "{n}" on the user's screen.
    #[test]
    fn placeholders_match_between_key_and_translation() {
        let keys = [
            "Дедлайн через {n} ч",
            "Дедлайн через {n} мин",
            "Начался блок (до {time})",
            "Помодоро запущено: {n} минут работы",
            "Перерыв {n} минут — отдохни",
            "Перерыв окончен: {n} минут работы",
            "{cat}: {mins} мин из {limit} сегодня",
            "Идёт таймер задачи «{task}», а вы в {app}.",
            "Скоро дедлайн задачи «{task}», а вы в {app}.",
            "Задача «{task}» в работе, а вы в {app}.",
            "Вы отсутствовали {n} мин. Продолжим задачу «{task}» или сделаем перерыв?",
            "Вы отсутствовали {n} мин. Ближайшая задача: {task}",
            "Вы отсутствовали {n} мин. С возвращением!",
            "до {time}",
            "{base} — осталось {n} мин",
        ];
        let names = |s: &str| {
            let mut v: Vec<String> = Vec::new();
            let mut rest = s;
            while let Some(i) = rest.find('{') {
                if let Some(j) = rest[i..].find('}') {
                    v.push(rest[i + 1..i + j].to_string());
                    rest = &rest[i + j + 1..];
                } else {
                    break;
                }
            }
            v.sort();
            v
        };
        for k in keys {
            let translated = en(k).unwrap_or_else(|| panic!("нет перевода для «{k}»"));
            assert_eq!(names(k), names(translated), "плейсхолдеры разошлись в «{k}»");
        }
    }

    // Keys actually used in the code must be present in the dictionary. The check
    // reads the sources: forgetting to add a translation after editing a string is
    // easy, and in English it silently degrades into Russian — that is the
    // mechanism working as designed, but for an already-localized place it is a bug.
    #[test]
    fn every_used_key_is_translated() {
        let sources = [
            include_str!("status.rs"),
            include_str!("notifier/scheduler.rs"),
            include_str!("notifier/pomodoro.rs"),
            include_str!("monitor/activity.rs"),
            include_str!("lib.rs"),
            // the context that goes into the AI prompt uses the same dictionary
            include_str!("commands/ai.rs"),
            include_str!("commands/planner.rs"),
        ];
        let mut missing: Vec<String> = Vec::new();
        for src in sources {
            for (idx, _) in src.match_indices("i18n::tr") {
                let rest = &src[idx..];
                let Some(q1) = rest.find('"') else { continue };
                let after = &rest[q1 + 1..];
                let Some(q2) = after.find('"') else { continue };
                let key = &after[..q2];
                // keys with no Cyrillic are not ours (argument names, for instance)
                if !key.chars().any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)) {
                    continue;
                }
                if en(key).is_none() {
                    missing.push(key.to_string());
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "нет перевода для: {missing:?}");
    }

    #[test]
    fn no_empty_translations() {
        for k in ["Дедлайн наступил!", "Выход", "Показать", "пауза"] {
            assert!(!tr(k, Lang::En).trim().is_empty(), "пустой перевод для «{k}»");
        }
    }

    // The CSS half of the "all comments in English" guard.
    //
    // Its twin lives in src/lib/comments.test.ts and covers .ts/.svelte/.rs plus
    // the root configs. Stylesheets could not join it: Vite runs .css through its
    // own pipeline, so import.meta.glob returns an EMPTY string for them under
    // every form of ?raw — the glob lists the path, and the guard then scans
    // nothing. That is the very failure mode this whole task exists to prevent, so
    // the check moved to the side that can actually read the file: include_str!,
    // the same bridge error.rs already uses to reach the frontend.
    #[test]
    fn no_russian_comments_in_css() {
        let css = include_str!("../../src/app.css");
        let mut offenders: Vec<String> = Vec::new();
        let mut in_comment = false;
        for (n, line) in css.lines().enumerate() {
            let body = if in_comment {
                line
            } else if let Some(i) = line.find("/*") {
                &line[i..]
            } else {
                ""
            };
            if body.contains("/*") && !body.contains("*/") {
                in_comment = true;
            } else if body.contains("*/") {
                in_comment = false;
            }
            // Quoted spans are cut out: a comment may legitimately quote a Russian
            // example without being written in Russian.
            let stripped: String = {
                let mut out = String::new();
                let mut quoted = false;
                for c in body.chars() {
                    match c {
                        '"' | '\'' | '«' | '»' => quoted = !quoted,
                        _ if !quoted => out.push(c),
                        _ => {}
                    }
                }
                out
            };
            if stripped.chars().any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё') {
                offenders.push(format!("src/app.css:{}  {}", n + 1, body.trim()));
            }
        }
        assert!(offenders.is_empty(), "русские комментарии в CSS:\n{}", offenders.join("\n"));
    }
}
