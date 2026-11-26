//! The Export menu in the Database Output header and the status bar's
//! running export, driven with the mouse through the real window.

use std::sync::Arc;

use boa_core::grid::csv::CsvDelimiter;
use boa_core::model::{QueryResult, ResultColumn};
use boa_core::state::csv_export as core;
use boa_core::tx::TxState;
use gpui::{Modifiers, TestAppContext, VisualTestContext};

use crate::app_state::AppState;
use crate::models::console::ConsoleResult;
use crate::tests::{TempDir, init, open_main};

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
}

fn shown(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    vcx.debug_bounds(selector).is_some()
}

/// The active console gets a two-row result, shown in the Database Output.
fn with_result(vcx: &mut VisualTestContext) -> String {
    let tab = vcx.update(|_, cx| {
        let state = AppState::global(cx).clone();
        let tab = state.workspaces.read(cx).active_tab().cloned().unwrap();
        let console = tab.console().unwrap().clone();
        console.update(cx, |c, cx| {
            c.result = Some(ConsoleResult {
                sql: "select id, name from t".into(),
                result: Arc::new(QueryResult {
                    columns: vec![
                        ResultColumn {
                            name: "id".into(),
                            ty: "int4".into(),
                        },
                        ResultColumn {
                            name: "name".into(),
                            ty: "text".into(),
                        },
                    ],
                    rows: vec![
                        vec![Some("1".into()), None],
                        vec![Some("2".into()), Some("x".into())],
                    ],
                    affected: None,
                    truncated: false,
                    pageable: true,
                    ms: 3,
                    tx: TxState::Idle,
                }),
                offset: 0,
                page_size: 500,
                ms: 3,
                run: 1,
            });
            cx.notify();
        });
        state
            .workspaces
            .update(cx, |w, cx| w.set_bottom_view(Some(tab.id.clone()), cx));
        tab.id.to_string()
    });
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    tab
}

#[gpui::test]
fn the_export_menu_sets_options_copies_and_exports(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-menu");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    with_result(&mut vcx);
    let exports = vcx.update(|_, cx| AppState::global(cx).exports.clone());

    click(&mut vcx, "bottom-export-trigger");
    assert!(shown(&mut vcx, "export-csv"), "the menu opens");
    // Options keep the menu open.
    click(&mut vcx, "export-delimiter-semicolon");
    click(&mut vcx, "export-header");
    let options = vcx.update(|_, cx| exports.read(cx).options());
    assert_eq!(options.delimiter, CsvDelimiter::Semicolon);
    assert!(!options.header);
    assert!(shown(&mut vcx, "export-csv"), "still open");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(core::FILE)).unwrap(),
        r#"{"delimiter":";","header":false}"#
    );

    // Copy page as CSV: with the options, then the menu closes.
    click(&mut vcx, "export-copy");
    assert_eq!(
        vcx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("1;\n2;x\n")
    );
    let title = vcx.update(|_, cx| {
        let state = AppState::global(cx);
        state
            .toasts
            .read(cx)
            .toasts()
            .last()
            .map(|t| t.title.to_string())
    });
    assert_eq!(title.as_deref(), Some("Copied 2 rows as CSV"));
    assert!(!shown(&mut vcx, "export-csv"), "closed");

    // Export to CSV…: the save panel.
    click(&mut vcx, "bottom-export-trigger");
    click(&mut vcx, "export-csv");
    assert!(vcx.did_prompt_for_new_path());
    vcx.simulate_new_path_selection(|_| None);
    assert!(!shown(&mut vcx, "export-csv"));
}

#[gpui::test]
fn a_running_export_shows_progress_and_cancel(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-running");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let tab = with_result(&mut vcx);
    assert!(shown(&mut vcx, "bottom-export-trigger"));
    assert!(!shown(&mut vcx, "status-export-cancel"));
    let exports = vcx.update(|_, cx| AppState::global(cx).exports.clone());
    vcx.update(|_, cx| {
        exports.update(cx, |e, cx| {
            let src = core::table_source("fake", "boa_dev", "public", "t");
            let id = e.jobs_mut().start(&tab, &src, "/tmp/t.csv");
            e.jobs_mut().progress(id, 1200, 65536);
            cx.notify();
        })
    });
    // The result toolbar says "Exporting…" with Cancel instead of the menu,
    // and the status bar shows the file and its progress.
    assert!(shown(&mut vcx, "bottom-export-cancel"));
    assert!(!shown(&mut vcx, "bottom-export-trigger"));
    assert!(shown(&mut vcx, "status-export-cancel"));
    vcx.update(|_, cx| {
        exports.update(cx, |e, cx| {
            let id = e.jobs()[0].id;
            e.jobs_mut().finish(id);
            cx.notify();
        })
    });
    assert!(shown(&mut vcx, "bottom-export-trigger"));
    assert!(!shown(&mut vcx, "status-export-cancel"));
}
