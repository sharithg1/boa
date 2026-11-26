//! Pending grid edits of table tabs, by tab id, with the page they apply to
//! (`state/edits.ts`), so switching tabs doesn't reload the rows under them.
//! Nothing reaches the database until a submit, which runs the generated
//! statements as one unit (the backend's `apply_edits`). Reloading, paging
//! or closing a tab with pending edits asks first ([`EditsState::confirm_discard`],
//! [`EditsState::close_guard`]).
//!
//! The TypeScript store was a zustand store with promises; here it is plain
//! state the app's model wraps. Submitting is split in two:
//! [`EditsState::begin_submit`] gives the statements to run, and
//! [`EditsState::finish_submit`] records how the run went. Logging to the
//! history and showing toasts stay with the caller; [`SubmitReport`] has
//! the texts.

use super::grid_edits::{EditStatement, EditTarget, Pending, build_statements};
use super::types::GridError;
use crate::edits::ApplyOutcome;
use crate::error::{CmdError, CmdResult};
use crate::model::{QueryResult, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A loaded page whose rows edits refer to (by index).
pub trait PageRows {
    fn rows(&self) -> &[Vec<Value>];
}

impl PageRows for Vec<Vec<Value>> {
    fn rows(&self) -> &[Vec<Value>] {
        self
    }
}

/// A page as the backend returns it (`Backend::run_query`): the app keeps
/// an `EditsState<QueryResult>`.
impl PageRows for QueryResult {
    fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }
}

/// The page a tab's edits apply to. Pages are compared by identity
/// (`Arc::ptr_eq`): a reloaded page starts the edits over.
#[derive(Debug)]
pub struct EditPage<R> {
    pub result: Arc<R>,
    pub offset: u64,
    pub row_limit: u64,
}

impl<R> Clone for EditPage<R> {
    fn clone(&self) -> Self {
        Self {
            result: Arc::clone(&self.result),
            offset: self.offset,
            row_limit: self.row_limit,
        }
    }
}

/// A tab's pending edits and their page.
#[derive(Debug, Clone)]
pub struct TabEdits<R> {
    pub pending: Pending,
    pub page: EditPage<R>,
}

/// "Discard N changes and `<action>`?": shown while a reload / page change /
/// close waits for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardPrompt {
    pub tab_id: String,
    /// What happens after discarding: "reload", "close the tab"…
    pub action: String,
    pub count: usize,
}

/// The action [`EditsState::close_guard`] asks about.
pub const CLOSE_TAB_ACTION: &str = "close the tab";

/// Failure of the last submit, shown in the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitError {
    /// The statement that failed; `None` when the submit failed as a whole.
    pub index: Option<usize>,
    pub message: String,
    pub detail: Option<String>,
}

/// An error as the UI shows it: `[SQLSTATE] message`, and detail / hint lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorText {
    pub message: String,
    pub detail: Option<String>,
}

/// Formats a backend error: a five-character SQLSTATE `code` prefixes the
/// message; `detail` and `hint` become "Detail: …" / "Hint: …" lines.
pub fn error_text(
    code: Option<&str>,
    message: &str,
    detail: Option<&str>,
    hint: Option<&str>,
) -> ErrorText {
    let sqlstate = code.filter(|c| {
        c.len() == 5
            && c.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
    });
    let lines: Vec<String> = [
        detail
            .filter(|d| !d.is_empty())
            .map(|d| format!("Detail: {d}")),
        hint.filter(|h| !h.is_empty()).map(|h| format!("Hint: {h}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    ErrorText {
        message: match sqlstate {
            Some(c) => format!("[{c}] {message}"),
            None => message.to_owned(),
        },
        detail: (!lines.is_empty()).then(|| lines.join("\n")),
    }
}

impl From<&CmdError> for ErrorText {
    fn from(e: &CmdError) -> Self {
        error_text(
            e.code.as_deref(),
            &e.message,
            e.detail.as_deref(),
            e.hint.as_deref(),
        )
    }
}

/// What [`EditsState::begin_submit`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitStart {
    /// No edits for the tab, or a submit is already running.
    Nothing,
    /// The edits were all no-ops: cleared, nothing to run (counts as success).
    Empty,
    /// Run these as one unit, then call [`EditsState::finish_submit`].
    Run(Vec<EditStatement>),
}

/// How running the statements went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// All applied; `in_transaction`: inside the user's open transaction.
    Applied { in_transaction: bool },
    /// Statement `index` failed; everything was rolled back.
    Failed { index: usize, error: ErrorText },
    /// The submit failed as a whole (connection lost…).
    Error(ErrorText),
}

/// The backend's `apply_edits` result: a failing statement (`failed`) or
/// an error of the call as a whole.
impl From<&CmdResult<ApplyOutcome>> for SubmitOutcome {
    fn from(r: &CmdResult<ApplyOutcome>) -> Self {
        match r {
            Ok(out) => match &out.failed {
                Some(f) => Self::Failed {
                    index: f.index,
                    error: ErrorText::from(&f.error),
                },
                None => Self::Applied {
                    in_transaction: out.in_transaction,
                },
            },
            Err(e) => Self::Error(ErrorText::from(e)),
        }
    }
}

/// A toast's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
    Error,
}

/// What to tell the user after a submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReport {
    pub ok: bool,
    pub kind: ToastKind,
    pub title: String,
    pub body: Option<String>,
    /// Failed statement: "Statement 2 of 3 failed; all changes were rolled
    /// back." (for the history entry's detail).
    pub note: Option<String>,
}

/// Pending edits of every table tab.
#[derive(Debug)]
pub struct EditsState<R> {
    tabs: HashMap<String, TabEdits<R>>,
    submitting: HashSet<String>,
    errors: HashMap<String, SubmitError>,
    discard: Option<DiscardPrompt>,
}

impl<R> Default for EditsState<R> {
    fn default() -> Self {
        Self {
            tabs: HashMap::new(),
            submitting: HashSet::new(),
            errors: HashMap::new(),
            discard: None,
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl<R: PageRows> EditsState<R> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The tab's edits, if it has any.
    pub fn tab(&self, tab_id: &str) -> Option<&TabEdits<R>> {
        self.tabs.get(tab_id)
    }

    /// The tab's pending edits of `page` (none if they belong to another page).
    pub fn pending_for(&self, tab_id: &str, page: &Arc<R>) -> Option<&Pending> {
        self.tabs
            .get(tab_id)
            .filter(|t| Arc::ptr_eq(&t.page.result, page))
            .map(|t| &t.pending)
    }

    /// Applies `f` to the tab's pending edits of `page` (starting over if
    /// the page changed). Clears the tab's submit error.
    pub fn update_pending(
        &mut self,
        tab_id: &str,
        page: EditPage<R>,
        f: impl FnOnce(&mut Pending),
    ) {
        let mut pending = match self.tabs.remove(tab_id) {
            Some(cur) if Arc::ptr_eq(&cur.page.result, &page.result) => cur.pending,
            _ => Pending::default(),
        };
        f(&mut pending);
        self.errors.remove(tab_id);
        if !pending.is_empty() {
            self.tabs
                .insert(tab_id.to_owned(), TabEdits { pending, page });
        }
    }

    /// Drops the tab's edits and submit error.
    pub fn clear_edits(&mut self, tab_id: &str) {
        self.tabs.remove(tab_id);
        self.errors.remove(tab_id);
    }

    /// Changed rows of the tab.
    pub fn pending_count(&self, tab_id: &str) -> usize {
        self.tabs
            .get(tab_id)
            .map_or(0, |t| t.pending.change_count())
    }

    pub fn is_submitting(&self, tab_id: &str) -> bool {
        self.submitting.contains(tab_id)
    }

    /// The last submit's failure, shown in the preview.
    pub fn error(&self, tab_id: &str) -> Option<&SubmitError> {
        self.errors.get(tab_id)
    }

    /// The open "discard changes?" prompt.
    pub fn discard_prompt(&self) -> Option<&DiscardPrompt> {
        self.discard.as_ref()
    }

    /// Before throwing away the tab's edits for `action`: true (edits
    /// cleared) to go on now; false when the user is asked first (see
    /// [`Self::discard_prompt`], answered by [`Self::resolve_discard`]). A
    /// prompt already open is dropped, as if cancelled.
    pub fn confirm_discard(&mut self, tab_id: &str, action: &str) -> bool {
        let count = self.pending_count(tab_id);
        if count == 0 {
            self.clear_edits(tab_id);
            return true;
        }
        self.discard = Some(DiscardPrompt {
            tab_id: tab_id.to_owned(),
            action: action.to_owned(),
            count,
        });
        false
    }

    /// The user answered the prompt: with `ok` the tab's edits are cleared.
    /// Returns the prompt, so the caller can go on with its action.
    pub fn resolve_discard(&mut self, ok: bool) -> Option<DiscardPrompt> {
        let prompt = self.discard.take()?;
        if ok {
            self.clear_edits(&prompt.tab_id);
        }
        Some(prompt)
    }

    /// Whether the tab may close now. With pending edits it asks first
    /// ([`CLOSE_TAB_ACTION`]); on a yes the caller closes the tab.
    pub fn close_guard(&mut self, tab_id: &str) -> bool {
        if !self.tabs.contains_key(tab_id) {
            return true;
        }
        self.confirm_discard(tab_id, CLOSE_TAB_ACTION)
    }

    /// Starts submitting the tab's edits: the statements to run as one unit.
    /// Fails when the target can't be edited (no primary key).
    pub fn begin_submit(
        &mut self,
        tab_id: &str,
        target: &EditTarget,
    ) -> Result<SubmitStart, GridError> {
        let Some(entry) = self.tabs.get(tab_id) else {
            return Ok(SubmitStart::Nothing);
        };
        if self.submitting.contains(tab_id) {
            return Ok(SubmitStart::Nothing);
        }
        let statements = build_statements(target, entry.page.result.rows(), &entry.pending)?;
        if statements.is_empty() {
            self.clear_edits(tab_id);
            return Ok(SubmitStart::Empty);
        }
        self.submitting.insert(tab_id.to_owned());
        Ok(SubmitStart::Run(statements))
    }

    /// Records how the submit of `count` statements went. On success the
    /// edits are cleared (the caller reloads the page); on failure
    /// everything was rolled back, the edits stay and the failing statement
    /// is remembered for the preview.
    pub fn finish_submit(
        &mut self,
        tab_id: &str,
        count: usize,
        outcome: SubmitOutcome,
    ) -> SubmitReport {
        self.submitting.remove(tab_id);
        match outcome {
            SubmitOutcome::Applied { in_transaction } => {
                self.clear_edits(tab_id);
                if in_transaction {
                    SubmitReport {
                        ok: true,
                        kind: ToastKind::Info,
                        title: format!(
                            "{count} change{} applied in the open transaction",
                            plural(count)
                        ),
                        body: Some("Commit the transaction to keep them.".into()),
                        note: None,
                    }
                } else {
                    SubmitReport {
                        ok: true,
                        kind: ToastKind::Success,
                        title: format!("Submitted {count} change{}", plural(count)),
                        body: None,
                        note: None,
                    }
                }
            }
            SubmitOutcome::Failed { index, error } => {
                let note = format!(
                    "Statement {} of {count} failed; all changes were rolled back.",
                    index + 1
                );
                self.errors.insert(
                    tab_id.to_owned(),
                    SubmitError {
                        index: Some(index),
                        message: error.message.clone(),
                        detail: error.detail,
                    },
                );
                SubmitReport {
                    ok: false,
                    kind: ToastKind::Error,
                    title: "Changes not submitted".into(),
                    body: Some(format!("{note}\n{}", error.message)),
                    note: Some(note),
                }
            }
            SubmitOutcome::Error(error) => {
                self.errors.insert(
                    tab_id.to_owned(),
                    SubmitError {
                        index: None,
                        message: error.message.clone(),
                        detail: error.detail,
                    },
                );
                SubmitReport {
                    ok: false,
                    kind: ToastKind::Error,
                    title: "Changes not submitted".into(),
                    body: Some(error.message),
                    note: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::grid_edits::RowRef;
    use super::*;

    type Rows = Vec<Vec<Value>>;

    fn page() -> EditPage<Rows> {
        EditPage {
            result: Arc::new(vec![
                vec![Some("1".into()), Some("ada".into())],
                vec![Some("2".into()), Some("grace".into())],
            ]),
            offset: 0,
            row_limit: 500,
        }
    }

    fn target() -> EditTarget {
        EditTarget {
            schema: "public".into(),
            table: "users".into(),
            columns: vec!["id".into(), "name".into()],
            pk: vec!["id".into()],
        }
    }

    fn edit(s: &mut EditsState<Rows>, p: &EditPage<Rows>, row: usize, v: &str) {
        let rows = Arc::clone(&p.result);
        s.update_pending("t1", p.clone(), |pending| {
            pending.set_cell(&rows, RowRef::Base(row), 1, Some(v.into()));
        });
    }

    #[test]
    fn keeps_edits_per_page_and_starts_over_on_a_new_page() {
        let mut s = EditsState::new();
        let p = page();
        edit(&mut s, &p, 0, "x");
        edit(&mut s, &p, 1, "y");
        assert_eq!(s.pending_count("t1"), 2);
        assert!(s.pending_for("t1", &p.result).is_some());
        let reloaded = page();
        assert!(s.pending_for("t1", &reloaded.result).is_none());
        edit(&mut s, &reloaded, 0, "z");
        assert_eq!(s.pending_count("t1"), 1);
    }

    #[test]
    fn drops_the_tab_when_its_edits_are_undone() {
        let mut s = EditsState::new();
        let p = page();
        edit(&mut s, &p, 0, "x");
        edit(&mut s, &p, 0, "ada");
        assert!(s.tab("t1").is_none());
    }

    #[test]
    fn asks_before_discarding_and_clears_on_yes() {
        let mut s = EditsState::new();
        assert!(s.confirm_discard("t1", "reload"));
        let p = page();
        edit(&mut s, &p, 0, "x");
        assert!(!s.confirm_discard("t1", "reload"));
        assert_eq!(
            s.discard_prompt(),
            Some(&DiscardPrompt {
                tab_id: "t1".into(),
                action: "reload".into(),
                count: 1
            })
        );
        let no = s.resolve_discard(false).unwrap();
        assert_eq!(no.action, "reload");
        assert_eq!(s.pending_count("t1"), 1);
        assert!(!s.close_guard("t1"));
        assert_eq!(s.discard_prompt().unwrap().action, CLOSE_TAB_ACTION);
        s.resolve_discard(true);
        assert_eq!(s.pending_count("t1"), 0);
        assert!(s.discard_prompt().is_none());
        assert!(s.close_guard("t1"));
    }

    #[test]
    fn submits_and_clears_on_success() {
        let mut s = EditsState::new();
        assert_eq!(s.begin_submit("t1", &target()), Ok(SubmitStart::Nothing));
        let p = page();
        edit(&mut s, &p, 1, "g");
        let SubmitStart::Run(statements) = s.begin_submit("t1", &target()).unwrap() else {
            panic!("expected statements");
        };
        assert_eq!(
            statements[0].sql,
            "UPDATE public.users SET name = 'g' WHERE id = '2'"
        );
        assert!(s.is_submitting("t1"));
        assert_eq!(s.begin_submit("t1", &target()), Ok(SubmitStart::Nothing));
        let report = s.finish_submit(
            "t1",
            1,
            SubmitOutcome::Applied {
                in_transaction: false,
            },
        );
        assert!(report.ok);
        assert_eq!(report.title, "Submitted 1 change");
        assert!(!s.is_submitting("t1"));
        assert_eq!(s.pending_count("t1"), 0);
    }

    #[test]
    fn keeps_edits_and_remembers_the_failing_statement() {
        let mut s = EditsState::new();
        let p = page();
        edit(&mut s, &p, 0, "x");
        edit(&mut s, &p, 1, "y");
        assert!(matches!(
            s.begin_submit("t1", &target()),
            Ok(SubmitStart::Run(_))
        ));
        let error = error_text(Some("23505"), "duplicate key", Some("Key (id)=(1)"), None);
        let report = s.finish_submit("t1", 2, SubmitOutcome::Failed { index: 1, error });
        assert!(!report.ok);
        assert_eq!(
            report.body.as_deref(),
            Some("Statement 2 of 2 failed; all changes were rolled back.\n[23505] duplicate key")
        );
        assert_eq!(
            s.error("t1"),
            Some(&SubmitError {
                index: Some(1),
                message: "[23505] duplicate key".into(),
                detail: Some("Detail: Key (id)=(1)".into())
            })
        );
        assert_eq!(s.pending_count("t1"), 2);
        // Editing again clears the error.
        edit(&mut s, &p, 0, "z");
        assert!(s.error("t1").is_none());
    }

    #[test]
    fn reports_changes_applied_in_an_open_transaction() {
        let mut s = EditsState::new();
        let p = page();
        edit(&mut s, &p, 0, "x");
        assert!(matches!(
            s.begin_submit("t1", &target()),
            Ok(SubmitStart::Run(_))
        ));
        let r = s.finish_submit(
            "t1",
            2,
            SubmitOutcome::Applied {
                in_transaction: true,
            },
        );
        assert_eq!(r.kind, ToastKind::Info);
        assert_eq!(r.title, "2 changes applied in the open transaction");
    }

    #[test]
    fn formats_backend_errors() {
        assert_eq!(
            error_text(Some("network"), "gone", None, Some("retry")),
            ErrorText {
                message: "gone".into(),
                detail: Some("Hint: retry".into())
            }
        );
        assert_eq!(
            error_text(Some("42P01"), "no table", None, None),
            ErrorText {
                message: "[42P01] no table".into(),
                detail: None
            }
        );
    }

    #[test]
    fn takes_the_backends_page_and_outcome() {
        use crate::edits::{ApplyFailure, StatementRun};
        use crate::model::ResultColumn;
        use crate::tx::TxState;

        let result = Arc::new(QueryResult {
            columns: vec![
                ResultColumn::new("id", "int4"),
                ResultColumn::new("name", "text"),
            ],
            rows: (*page().result).clone(),
            affected: Some(2),
            truncated: false,
            pageable: true,
            ms: 1,
            tx: TxState::Idle,
        });
        let p = EditPage {
            result: Arc::clone(&result),
            offset: 0,
            row_limit: 500,
        };
        let mut s: EditsState<QueryResult> = EditsState::new();
        s.update_pending("t1", p, |pending| {
            pending.set_cell(&result.rows, RowRef::Base(1), 1, Some("hopper".into()));
        });
        let SubmitStart::Run(statements) = s.begin_submit("t1", &target()).unwrap() else {
            panic!("nothing to run");
        };
        assert_eq!(
            statements[0].sql,
            "UPDATE public.users SET name = 'hopper' WHERE id = '2'"
        );
        let wire: Vec<crate::edits::EditStatement> = statements.iter().map(Into::into).collect();
        assert_eq!(wire[0].sql, statements[0].sql);
        assert!(wire[0].exactly_one);

        let outcome = |failed, in_transaction| -> CmdResult<ApplyOutcome> {
            Ok(ApplyOutcome {
                runs: vec![StatementRun { affected: 1, ms: 0 }],
                failed,
                in_transaction,
                tx: TxState::Idle,
                ms: 0,
            })
        };
        assert_eq!(
            SubmitOutcome::from(&outcome(None, true)),
            SubmitOutcome::Applied {
                in_transaction: true
            }
        );
        let error = CmdError {
            message: "boom".into(),
            code: Some("23505".into()),
            hint: Some("try again".into()),
            ..CmdError::default()
        };
        assert_eq!(
            SubmitOutcome::from(&outcome(
                Some(ApplyFailure {
                    index: 0,
                    error: error.clone(),
                }),
                false
            )),
            SubmitOutcome::Failed {
                index: 0,
                error: ErrorText {
                    message: "[23505] boom".into(),
                    detail: Some("Hint: try again".into()),
                },
            }
        );
        assert_eq!(
            SubmitOutcome::from(&Err(CmdError::code("not_connected", "gone"))),
            SubmitOutcome::Error(ErrorText {
                message: "gone".into(),
                detail: None,
            })
        );
        let report = s.finish_submit("t1", 1, SubmitOutcome::from(&outcome(None, false)));
        assert_eq!(report.title, "Submitted 1 change");
    }
}
