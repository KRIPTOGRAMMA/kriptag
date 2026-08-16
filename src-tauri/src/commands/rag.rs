// Ask-your-notes: a question goes to the note search, the notes it finds are
// handed to the model as context, and the answer comes back with the list of
// notes it was built from.
//
// Retrieval is search_notes_impl and nothing else. That is deliberate: the chat
// then inherits every improvement to search for free — the stemming added in
// v0.10.19 already means "покупки" reaches a note that says "покупками", and
// embeddings later would raise the chat's ceiling without touching this file.
//
// What this CANNOT do, and why it is built to admit it: the retrieval is
// lexical, so a question phrased in words the note does not contain finds
// nothing ("выгорание" will not reach "устал, нет сил"). Closing that gap needs
// embeddings, which is a separate decision with a separate cost. So an empty
// retrieval is a first-class answer here — the model is told to say it found
// nothing rather than to invent something, and the sources are always shown so
// the user can see what the answer rests on.

use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{Emitter, Manager};

use crate::commands::ai::{Prompt, ask_ai_localized};
use crate::commands::notes::{Note, search_notes_impl};

/// How many notes go into the context.
///
/// Small on purpose: the local model is a 0.5B by default, and its context
/// window is not large. Five whole notes already risk crowding out the question
/// itself, and a model that loses the question answers something else.
const MAX_NOTES: usize = 5;

/// How much of one note is passed.
///
/// A long note would eat the whole budget by itself and push the others out,
/// so each is capped and the cut is marked. The opening of a note is the part
/// most likely to say what it is about.
const MAX_CHARS_PER_NOTE: usize = 1500;

const SYSTEM_ASK_NOTES: Prompt = Prompt {
    ru: "Ты помощник, отвечающий по личным заметкам пользователя. Отвечай ТОЛЬКО по тексту заметок ниже. \
Если в них нет ответа, так и скажи — не придумывай и не добавляй знания извне. \
Ссылайся на заметки по их названиям. Отвечай кратко, обычным текстом.",
    en: "You answer questions from the user's personal notes. Use ONLY the notes below. \
If they do not contain the answer, say so — do not invent anything and do not add outside knowledge. \
Refer to notes by their titles. Answer briefly, in plain text.",
};

#[derive(Clone, Serialize)]
pub struct AskNotesSource {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Serialize)]
pub struct AskNotesPayload {
    pub request_id: String,
    pub result: Option<String>,
    pub sources: Vec<AskNotesSource>,
    pub error: Option<String>,
}

/// Trims one note to the budget, marking the cut so the model does not treat a
/// truncated sentence as the end of the thought.
fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_CHARS_PER_NOTE {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_CHARS_PER_NOTE).collect();
    format!("{head}…")
}

/// Lays the retrieved notes out for the model.
///
/// Titles are kept because the prompt asks the model to cite them: without the
/// title in the context there is nothing for it to cite, and it starts making
/// references up.
pub fn build_context(notes: &[Note], question: &str) -> String {
    let mut out = String::new();
    for (i, note) in notes.iter().enumerate() {
        out.push_str(&format!("--- Заметка {} — «{}» ---\n{}\n\n", i + 1, note.title, clip(&note.content)));
    }
    out.push_str(&format!("Вопрос: {question}"));
    out
}

/// The answer given when retrieval came back empty.
///
/// Handled here rather than by the model: with nothing in the context there is
/// nothing to ground an answer in, and asking a 0.5B to stay silent about a
/// topic it half-knows is exactly the situation where it invents. Not calling
/// the model at all is the only reliable way to not get a made-up answer.
pub const NOTHING_FOUND_RU: &str =
    "В заметках ничего не нашлось по этому вопросу. Поиск идёт по словам, поэтому попробуйте другую формулировку.";
pub const NOTHING_FOUND_EN: &str =
    "Nothing in your notes matches this question. The search goes by words, so try phrasing it differently.";

#[tauri::command]
pub async fn ai_ask_notes(app: tauri::AppHandle, request_id: String, question: String) -> Result<(), String> {
    tokio::spawn(async move {
        let payload = match run_ask(&app, &question).await {
            Ok((result, sources)) => AskNotesPayload { request_id, result: Some(result), sources, error: None },
            Err(e) => AskNotesPayload { request_id, result: None, sources: vec![], error: Some(e) },
        };
        let _ = app.emit("ai-ask-notes", payload);
    });
    Ok(())
}

async fn run_ask(app: &tauri::AppHandle, question: &str) -> Result<(String, Vec<AskNotesSource>), String> {
    let pool = app.state::<SqlitePool>();
    let lang = crate::i18n::current_lang(pool.inner()).await;

    let notes = retrieve(pool.inner(), question).await?;

    if notes.is_empty() {
        let msg = match lang {
            crate::i18n::Lang::Ru => NOTHING_FOUND_RU,
            crate::i18n::Lang::En => NOTHING_FOUND_EN,
        };
        return Ok((msg.to_string(), vec![]));
    }

    let sources: Vec<AskNotesSource> = notes
        .iter()
        .map(|n| AskNotesSource { id: n.id.clone(), title: n.title.clone() })
        .collect();

    let context = build_context(&notes, question);
    let answer = ask_ai_localized(app, &SYSTEM_ASK_NOTES, &context).await?;

    Ok((answer.trim().to_string(), sources))
}

/// The retrieval step, kept separate so it can be tested against a real database
/// without a model behind it.
pub async fn retrieve(pool: &SqlitePool, question: &str) -> Result<Vec<Note>, String> {
    let mut found = search_notes_impl(pool, question.to_string())
        .await
        .map_err(|e| e.to_string())?;
    found.truncate(MAX_NOTES);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, content: &str) -> Note {
        Note {
            id: format!("id-{title}"),
            title: title.into(),
            content: content.into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
            pinned: false,
            created_at: String::new(),
            updated_at: String::new(),
            reminder_at: None,
        }
    }

    #[test]
    fn context_carries_titles_so_the_model_can_cite_them() {
        let ctx = build_context(&[note("Покупки", "молоко и хлеб")], "что купить?");
        assert!(ctx.contains("Покупки"), "нет названия — цитировать модели нечего: {ctx}");
        assert!(ctx.contains("молоко и хлеб"), "нет текста заметки: {ctx}");
    }

    #[test]
    fn the_question_survives_the_context() {
        // A question crowded out by the notes is the classic failure: the model
        // answers about the text in general instead of what was asked.
        let ctx = build_context(&[note("а", "текст")], "когда встреча?");
        assert!(ctx.contains("когда встреча?"), "вопрос потерялся: {ctx}");
    }

    #[test]
    fn a_long_note_is_clipped_and_the_cut_is_marked() {
        let long = "я".repeat(MAX_CHARS_PER_NOTE + 500);
        let ctx = build_context(&[note("длинная", &long)], "?");
        assert!(ctx.contains('…'), "обрезка не помечена — модель примет обрыв за конец мысли");
        assert!(
            ctx.chars().count() < MAX_CHARS_PER_NOTE + 500,
            "длинная заметка попала целиком и вытеснит остальные"
        );
    }

    #[test]
    fn a_short_note_is_passed_whole() {
        let ctx = build_context(&[note("к", "коротко")], "?");
        assert!(ctx.contains("коротко"));
        assert!(!ctx.contains('…'), "короткую заметку обрезать нечего");
    }

    #[test]
    fn every_retrieved_note_reaches_the_context() {
        let notes: Vec<Note> = (0..MAX_NOTES).map(|i| note(&format!("з{i}"), &format!("текст{i}"))).collect();
        let ctx = build_context(&notes, "?");
        for i in 0..MAX_NOTES {
            assert!(ctx.contains(&format!("текст{i}")), "заметка {i} не попала в контекст");
        }
    }

    // --- Retrieval against a real database ---

    use crate::commands::notes::{CreateNote, create_note_impl, delete_note_impl};

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./src/db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn add(pool: &SqlitePool, title: &str, content: &str) -> Note {
        create_note_impl(pool, CreateNote {
            title: title.into(),
            content: content.into(),
            tags: vec![],
            linked_task_id: None,
            project_id: None,
        }).await.unwrap()
    }

    #[tokio::test]
    async fn retrieval_finds_the_note_the_question_is_about() {
        let pool = test_pool().await;
        add(&pool, "поездка", "выехать в субботу, забрать билеты").await;
        add(&pool, "рецепт", "тесто, начинка, духовка").await;

        let got = retrieve(&pool, "билеты").await.unwrap();
        assert_eq!(got.len(), 1, "нашлось не то: {:?}", got.iter().map(|n| &n.title).collect::<Vec<_>>());
        assert_eq!(got[0].title, "поездка");
    }

    #[tokio::test]
    async fn retrieval_inherits_stemming_from_search() {
        // The point of building on search_notes_impl rather than a query of its
        // own: v0.10.19 taught search about word forms, and the chat got it for
        // free. If retrieval is ever rewritten to query FTS directly, this fails.
        let pool = test_pool().await;
        add(&pool, "хозяйство", "сходить за покупками").await;

        let got = retrieve(&pool, "покупки").await.unwrap();
        assert_eq!(got.len(), 1, "форма слова не дошла до ретривала — связь с поиском потеряна");
    }

    #[tokio::test]
    async fn retrieval_is_capped_so_the_question_is_not_crowded_out() {
        let pool = test_pool().await;
        for i in 0..(MAX_NOTES + 3) {
            add(&pool, &format!("заметка {i}"), "общее слово барсук").await;
        }

        let got = retrieve(&pool, "барсук").await.unwrap();
        assert_eq!(got.len(), MAX_NOTES, "в контекст пойдёт больше заметок, чем влезает");
    }

    #[tokio::test]
    async fn a_question_matching_nothing_retrieves_nothing() {
        // The honest-empty case: this is what makes the chat say "not found"
        // instead of inventing. If retrieval ever returns something here, the
        // model gets unrelated notes and answers from them.
        let pool = test_pool().await;
        add(&pool, "поездка", "выехать в субботу").await;

        let got = retrieve(&pool, "квантовая хромодинамика").await.unwrap();
        assert!(got.is_empty(), "нашлось нерелевантное: {:?}", got.iter().map(|n| &n.title).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn a_trashed_note_never_reaches_the_model() {
        let pool = test_pool().await;
        let note = add(&pool, "выброшенная", "секретное слово барсук").await;
        delete_note_impl(&pool, note.id).await.unwrap();

        let got = retrieve(&pool, "барсук").await.unwrap();
        assert!(got.is_empty(), "заметка из Корзины ушла бы в контекст модели");
    }
}
