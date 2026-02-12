use crate::{
    ui::Content,
    widgets::{Loading, LoadingWidget, Terminal},
};
use bevy_ecs::prelude::*;

pub fn render_error(mut terminal: ResMut<Terminal>, err: Res<Content<'static>>) {
    _ = terminal.draw(|frame| {
        frame.render_widget(err.clone(), frame.size());
    });
}

pub fn render_loading(mut terminal: ResMut<Terminal>, loading: Res<Loading>) {
    _ = terminal.draw(|frame| {
        frame.render_widget(LoadingWidget::from(&*loading), frame.size());
    });
}

pub fn error(mut terminal: ResMut<Terminal>, err: Res<Content<'static>>) {
    _ = terminal.draw(|frame| {
        frame.render_widget(err.clone(), frame.size());
    });
}

pub fn loading(mut terminal: ResMut<Terminal>, loading: Res<Loading>) {
    _ = terminal.draw(|frame| {
        frame.render_widget(LoadingWidget::from(&*loading), frame.size());
    });
}
