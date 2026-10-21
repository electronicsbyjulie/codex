//! Final text checks for terminal frames and scrollback, including local UI text.

use codex_config::output_filter::OutputFilter;
use codex_config::output_filter::global;
use ratatui::buffer::Buffer;
use ratatui::text::Line;

use crate::terminal_hyperlinks::HyperlinkLine;

pub(crate) fn buffer(buffer: &mut Buffer) {
    if let Some(filter) = global() {
        mask_buffer(buffer, filter);
    }
}

fn mask_buffer(buffer: &mut Buffer, filter: &OutputFilter) {
    let mut text = String::new();
    let mut cells = Vec::with_capacity(buffer.content.len());
    for cell in &buffer.content {
        let start = text.len();
        let symbol = crate::custom_terminal::osc8_hyperlink_parts(cell.symbol())
            .map_or(cell.symbol(), |(_, visible)| visible);
        text.push_str(symbol);
        cells.push(start..text.len());
    }
    let matches = filter.ranges(&text);
    for (cell, range) in buffer.content.iter_mut().zip(cells) {
        if matches
            .iter()
            .any(|found| found.start < range.end && range.start < found.end)
        {
            // Keep cell positions stable, including styled or multi-byte graphemes.
            cell.set_symbol(" ");
        } else if let Some((_, visible)) =
            crate::custom_terminal::osc8_hyperlink_parts(cell.symbol())
        {
            // Metadata can also appear on hover. Drop OSC links in filtered mode.
            let visible = visible.to_owned();
            cell.set_symbol(&visible);
        }
    }
}

pub(crate) fn history(lines: &[HyperlinkLine]) -> Option<Vec<HyperlinkLine>> {
    let filter = global()?;
    Some(
        lines
            .iter()
            .map(|line| {
                let mut line = line.clone();
                let text: String = line
                    .line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                if filter.is_match(&text) {
                    line.line = Line::from(filter.text(&text)).style(line.line.style);
                }
                // Reflow must use the filtered line, never an unfiltered source snapshot.
                line.source = None;
                line.hyperlinks.clear();
                line
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::output_filter::OutputFilterConfig;
    use codex_config::output_filter::OutputFilterRule;
    use pretty_assertions::assert_eq;
    use ratatui::layout::Rect;

    #[test]
    fn matches_across_cells_and_styles_are_masked() {
        let filter = OutputFilter::compile(OutputFilterConfig {
            rules: vec![OutputFilterRule {
                pattern: "secret".into(),
                exempt_pattern: None,
                replacement: "[filtered]".into(),
            }],
        })
        .unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 1));
        for (cell, symbol) in buffer.content.iter_mut().zip("a secret b".chars()) {
            cell.set_symbol(&symbol.to_string());
        }
        mask_buffer(&mut buffer, &filter);
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert_eq!(text, "a        b");
    }
}
