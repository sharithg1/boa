//! The grid's development hooks without the `dev-hooks` feature: the same
//! API as `dev.rs`, doing nothing, so release builds read no `BOA_GRID_*`
//! variables.

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Context, Window};

use super::ResultGrid;

/// Dev-hook state kept in the grid (empty).
#[derive(Debug, Default)]
pub(super) struct DevState;

impl DevState {
    pub fn from_env() -> Self {
        Self
    }

    pub fn add_build_time(&mut self, _: Duration) {}

    pub fn render_started(&mut self) {}

    pub fn paint_marker(&self) -> Option<Rc<StdCell<Option<Instant>>>> {
        None
    }
}

pub(super) fn schedule_quit(_: &mut Context<ResultGrid>) {}

pub(super) fn start(_: &mut ResultGrid, _: &mut Window, _: &mut Context<ResultGrid>) {}

pub(super) fn after_load(_: &mut ResultGrid) {}

pub(super) fn on_first_paint(_: &mut ResultGrid, _: &mut Window, _: &mut Context<ResultGrid>) {}
