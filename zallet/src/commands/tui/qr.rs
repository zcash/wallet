//! Rendering of strings (addresses) as terminal QR codes.

use qrcode::{EcLevel, QrCode};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// A rendered QR code, with the terminal dimensions it occupies.
pub(super) struct RenderedQr {
    pub(super) lines: Vec<Line<'static>>,
    /// Width in terminal columns.
    pub(super) width: u16,
    /// Height in terminal rows.
    pub(super) height: u16,
}

/// Renders the given data as a QR code, returning one [`Line`] per two rows of QR modules.
///
/// Two vertical modules are packed into each character cell using the unicode half-block
/// characters, so the resulting QR code is roughly square in a typical terminal. Returns
/// `None` if the data cannot be encoded as a QR code (e.g. it is too long).
pub(super) fn render(data: &str) -> Option<RenderedQr> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M).ok()?;
    let width = code.width();
    let modules = code.to_colors();

    // `dark(x, y)` is true when the module is dark (should be drawn).
    let dark = |x: usize, y: usize| -> bool {
        if x >= width || y >= width {
            // Treat out-of-bounds (the odd row of an odd-height code) as light.
            false
        } else {
            modules[y * width + x] == qrcode::Color::Dark
        }
    };

    // One character of horizontal quiet zone on each side.
    const QUIET: usize = 1;

    let mut lines = Vec::with_capacity(width / 2 + 1 + QUIET * 2);

    // Explicitly pin dark modules to black and light modules to white, rather than relying
    // on the terminal's default colors. The half-block glyphs encode the top module in the
    // cell's foreground and the bottom module in its background, so with `fg = black` and
    // `bg = white` every module renders dark-on-light regardless of the terminal's theme
    // (a light-on-dark terminal would otherwise produce an inverted code that some scanners
    // reject).
    let style = Style::default().fg(Color::Black).bg(Color::White);

    // Top quiet zone (one blank line covers two module rows).
    let total_width = width + QUIET * 2;
    let blank: String = " ".repeat(total_width);
    lines.push(Line::from(Span::styled(blank.clone(), style)));

    let mut y = 0;
    while y < width {
        let mut s = String::with_capacity(total_width);
        // Left quiet zone.
        for _ in 0..QUIET {
            s.push(' ');
        }
        for x in 0..width {
            let top = dark(x, y);
            let bottom = dark(x, y + 1);
            // Foreground = dark module. Use half blocks so two rows fit one cell.
            s.push(match (top, bottom) {
                (true, true) => '\u{2588}',  // full block
                (true, false) => '\u{2580}', // upper half block
                (false, true) => '\u{2584}', // lower half block
                (false, false) => ' ',
            });
        }
        for _ in 0..QUIET {
            s.push(' ');
        }
        lines.push(Line::from(Span::styled(s, style)));
        y += 2;
    }

    // Bottom quiet zone.
    lines.push(Line::from(Span::styled(blank, style)));

    let height = lines.len() as u16;
    Some(RenderedQr {
        lines,
        width: total_width as u16,
        height,
    })
}
