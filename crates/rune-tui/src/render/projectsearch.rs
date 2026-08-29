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
    let spans = crate::render::search::build_spans(
        &state.query,
        None,
        true,
        area.width as usize,
        &app.theme,
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), bar_area);
}
