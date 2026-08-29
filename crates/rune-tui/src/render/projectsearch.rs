use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::App;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.projectsearch() else {
        return;
    };
    if area.height == 0 {
        return;
    }

    let bar_area = Rect::new(area.x, area.y, area.width, 1);
    let readout = readout_text(app);
    let spans = crate::render::search::build_spans(
        &state.query,
        readout.as_deref(),
        true,
        area.width as usize,
        &app.theme,
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), bar_area);
}

fn readout_text(app: &App) -> Option<String> {
    let index = app.project_index.as_ref()?;
    if !index.building {
        return None;
    }
    Some(crate::projectsearch::spinner_char(index.spinner_frame).to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    #[test]
    fn the_readout_shows_one_spinner_char_only_while_the_build_is_in_flight() {
        let mut app = crate::app::App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        let mut effects = crate::runtime::Effects::default();
        crate::projectsearch::open(&mut app, &mut effects);

        let building = readout_text(&app).expect("a build is in flight right after open");
        assert_eq!(building.chars().count(), 1);
        assert!(('\u{2800}'..='\u{28FF}').contains(&building.chars().next().unwrap()));

        if let Some(index) = app.project_index.as_mut() {
            index.building = false;
        }
        assert_eq!(readout_text(&app), None, "an idle index draws no spinner");
    }
}
