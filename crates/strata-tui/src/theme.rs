//! Colours, from rcmd's built-in themes: `mc` (the default), `dark` and
//! `bw`, chosen by `theme = "..."` in the config.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    /// The type being shown, in the list of types.
    pub current_fg: Color,
    pub header_fg: Color,
    pub select_bg: Color,
    pub select_fg: Color,
    /// A vault item seen through a locked vault.
    pub locked_fg: Color,
    pub dialog_bg: Color,
    pub dialog_fg: Color,
    pub error_bg: Color,
    pub error_fg: Color,
    pub prompt_fg: Color,
    pub key_fg: Color,
    pub key_bg: Color,
    pub label_fg: Color,
    pub label_bg: Color,
    /// bw draws the cursor reversed rather than in colour.
    pub reverse_select: bool,
}

impl Theme {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "mc" => Some(Self::mc()),
            "dark" => Some(Self::dark()),
            "bw" => Some(Self::bw()),
            _ => None,
        }
    }

    pub fn mc() -> Self {
        Self {
            bg: Color::Blue,
            fg: Color::Gray,
            current_fg: Color::White,
            header_fg: Color::Yellow,
            select_bg: Color::Cyan,
            select_fg: Color::Black,
            locked_fg: Color::LightRed,
            dialog_bg: Color::Gray,
            dialog_fg: Color::Black,
            error_bg: Color::Red,
            error_fg: Color::White,
            prompt_fg: Color::LightCyan,
            key_fg: Color::White,
            key_bg: Color::Black,
            label_fg: Color::Black,
            label_bg: Color::Cyan,
            reverse_select: false,
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: Color::Rgb(0x1e, 0x22, 0x2a),
            fg: Color::Rgb(0xc8, 0xcc, 0xd4),
            current_fg: Color::Rgb(0x61, 0xaf, 0xef),
            header_fg: Color::Rgb(0xe5, 0xc0, 0x7b),
            select_bg: Color::Rgb(0x3e, 0x44, 0x51),
            select_fg: Color::Rgb(0xff, 0xff, 0xff),
            locked_fg: Color::Rgb(0xe0, 0x6c, 0x75),
            dialog_bg: Color::Rgb(0x2c, 0x31, 0x3c),
            dialog_fg: Color::Rgb(0xc8, 0xcc, 0xd4),
            error_bg: Color::Rgb(0xbe, 0x50, 0x46),
            error_fg: Color::Rgb(0xff, 0xff, 0xff),
            prompt_fg: Color::Rgb(0x56, 0xb6, 0xc2),
            key_fg: Color::Rgb(0xc8, 0xcc, 0xd4),
            key_bg: Color::Rgb(0x1e, 0x22, 0x2a),
            label_fg: Color::Rgb(0x1e, 0x22, 0x2a),
            label_bg: Color::Rgb(0x56, 0xb6, 0xc2),
            reverse_select: false,
        }
    }

    pub fn bw() -> Self {
        let r = Color::Reset;
        Self {
            bg: r,
            fg: r,
            current_fg: r,
            header_fg: r,
            select_bg: r,
            select_fg: r,
            locked_fg: r,
            dialog_bg: r,
            dialog_fg: r,
            error_bg: r,
            error_fg: r,
            prompt_fg: r,
            key_fg: r,
            key_bg: r,
            label_fg: r,
            label_bg: r,
            reverse_select: true,
        }
    }

    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    pub fn selected(&self) -> Style {
        if self.reverse_select {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(self.select_fg).bg(self.select_bg)
        }
    }

    pub fn header(&self) -> Style {
        Style::new()
            .fg(self.header_fg)
            .bg(self.bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn dialog(&self) -> Style {
        Style::new().fg(self.dialog_fg).bg(self.dialog_bg)
    }

    pub fn error(&self) -> Style {
        Style::new().fg(self.error_fg).bg(self.error_bg)
    }
}
