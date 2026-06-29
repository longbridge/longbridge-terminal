#[macro_use]
mod macros;

pub mod app;
pub(crate) mod input;
pub(crate) mod keys;
pub(crate) mod kline;
pub(crate) mod mouse;
pub(crate) mod nav;
pub mod popup;
pub mod render;
pub mod systems;
pub mod ui;
mod views;
pub mod widgets;

pub(crate) fn schema_for_path(path: &[String]) -> Option<crate::cli::schema::ResponseSchema> {
    (path == ["tui"]).then(|| crate::cli::schema::text("Interactive terminal UI session"))
}
