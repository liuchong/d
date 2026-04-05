//! Terminal color utilities
//!
//! Provides color support detection and ANSI color codes.

/// ANSI color codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    // Standard colors
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    // Bright colors
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    // RGB
    Rgb(u8, u8, u8),
}

impl Color {
    /// Get ANSI foreground code
    pub fn fg_code(&self) -> String {
        match self {
            Color::Black => "\x1b[30m".to_string(),
            Color::Red => "\x1b[31m".to_string(),
            Color::Green => "\x1b[32m".to_string(),
            Color::Yellow => "\x1b[33m".to_string(),
            Color::Blue => "\x1b[34m".to_string(),
            Color::Magenta => "\x1b[35m".to_string(),
            Color::Cyan => "\x1b[36m".to_string(),
            Color::White => "\x1b[37m".to_string(),
            Color::BrightBlack => "\x1b[90m".to_string(),
            Color::BrightRed => "\x1b[91m".to_string(),
            Color::BrightGreen => "\x1b[92m".to_string(),
            Color::BrightYellow => "\x1b[93m".to_string(),
            Color::BrightBlue => "\x1b[94m".to_string(),
            Color::BrightMagenta => "\x1b[95m".to_string(),
            Color::BrightCyan => "\x1b[96m".to_string(),
            Color::BrightWhite => "\x1b[97m".to_string(),
            Color::Rgb(r, g, b) => format!("\x1b[38;2;{};{};{}m", r, g, b),
        }
    }

    /// Get ANSI background code
    pub fn bg_code(&self) -> String {
        match self {
            Color::Black => "\x1b[40m".to_string(),
            Color::Red => "\x1b[41m".to_string(),
            Color::Green => "\x1b[42m".to_string(),
            Color::Yellow => "\x1b[43m".to_string(),
            Color::Blue => "\x1b[44m".to_string(),
            Color::Magenta => "\x1b[45m".to_string(),
            Color::Cyan => "\x1b[46m".to_string(),
            Color::White => "\x1b[47m".to_string(),
            Color::BrightBlack => "\x1b[100m".to_string(),
            Color::BrightRed => "\x1b[101m".to_string(),
            Color::BrightGreen => "\x1b[102m".to_string(),
            Color::BrightYellow => "\x1b[103m".to_string(),
            Color::BrightBlue => "\x1b[104m".to_string(),
            Color::BrightMagenta => "\x1b[105m".to_string(),
            Color::BrightCyan => "\x1b[106m".to_string(),
            Color::BrightWhite => "\x1b[107m".to_string(),
            Color::Rgb(r, g, b) => format!("\x1b[48;2;{};{};{}m", r, g, b),
        }
    }
}

/// Reset code
pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const ITALIC: &str = "\x1b[3m";
pub const UNDERLINE: &str = "\x1b[4m";

/// Styled text
pub struct Styled<'a> {
    text: &'a str,
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

impl<'a> Styled<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
        }
    }

    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

impl<'a> std::fmt::Display for Styled<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !colors_enabled() {
            return write!(f, "{}", self.text);
        }

        let mut result = String::new();

        if self.bold {
            result.push_str(BOLD);
        }
        if self.dim {
            result.push_str(DIM);
        }
        if self.italic {
            result.push_str(ITALIC);
        }
        if self.underline {
            result.push_str(UNDERLINE);
        }
        if let Some(ref fg) = self.fg {
            result.push_str(&fg.fg_code());
        }
        if let Some(ref bg) = self.bg {
            result.push_str(&bg.bg_code());
        }

        if result.is_empty() {
            write!(f, "{}", self.text)
        } else {
            write!(f, "{}{}{}", result, self.text, RESET)
        }
    }
}

/// Check if colors are enabled
fn colors_enabled() -> bool {
    // Check NO_COLOR environment variable
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }

    // Check FORCE_COLOR environment variable
    if std::env::var("FORCE_COLOR").is_ok() {
        return true;
    }

    // Check if stdout is a tty
    atty::is(atty::Stream::Stdout)
}

/// Convenience functions for colored text
pub fn red(text: &str) -> Styled {
    Styled::new(text).fg(Color::Red)
}

pub fn green(text: &str) -> Styled {
    Styled::new(text).fg(Color::Green)
}

pub fn yellow(text: &str) -> Styled {
    Styled::new(text).fg(Color::Yellow)
}

pub fn blue(text: &str) -> Styled {
    Styled::new(text).fg(Color::Blue)
}

pub fn magenta(text: &str) -> Styled {
    Styled::new(text).fg(Color::Magenta)
}

pub fn cyan(text: &str) -> Styled {
    Styled::new(text).fg(Color::Cyan)
}

pub fn bright_green(text: &str) -> Styled {
    Styled::new(text).fg(Color::BrightGreen)
}

pub fn bright_yellow(text: &str) -> Styled {
    Styled::new(text).fg(Color::BrightYellow)
}

pub fn bright_blue(text: &str) -> Styled {
    Styled::new(text).fg(Color::BrightBlue)
}

pub fn bold(text: &str) -> Styled {
    Styled::new(text).bold()
}

/// Colorize based on content type
pub fn colorize_tool_name(name: &str) -> String {
    format!("{}", Styled::new(name).fg(Color::BrightCyan).bold())
}

pub fn colorize_tool_result(result: &str) -> String {
    if result.starts_with("Error") || result.contains("failed") {
        format!("{}", red(result))
    } else {
        format!("{}", green(result))
    }
}

pub fn colorize_file_path(path: &str) -> String {
    format!("{}", Styled::new(path).fg(Color::BrightBlue).underline())
}

pub fn colorize_code(code: &str, lang: &str) -> String {
    // Simple syntax highlighting
    let highlighted = match lang {
        "rust" => highlight_rust(code),
        _ => code.to_string(),
    };
    format!("{}", Styled::new(&highlighted).fg(Color::BrightWhite))
}

fn highlight_rust(code: &str) -> String {
    // Very basic Rust highlighting
    let keywords = ["fn", "let", "mut", "pub", "use", "mod", "struct", "impl", "if", "else", "return"];
    let mut result = code.to_string();
    
    for kw in &keywords {
        let colored = format!("{}\x1b[0m", Styled::new(kw).fg(Color::BrightMagenta));
        result = result.replace(&format!(" {} ", kw), &format!(" {} ", colored));
    }
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_codes() {
        assert_eq!(Color::Red.fg_code(), "\x1b[31m");
        assert_eq!(Color::Green.bg_code(), "\x1b[42m");
    }

    #[test]
    fn test_styled_structure() {
        let styled = Styled::new("test").fg(Color::Red).bold();
        // Just verify the struct is created correctly
        assert_eq!(styled.text, "test");
        assert!(styled.bold);
        assert!(matches!(styled.fg, Some(Color::Red)));
    }

    #[test]
    fn test_convenience_functions() {
        // Just verify the functions create styled text
        let _red = red("error");
        let _green = green("ok");
        let _yellow = yellow("warn");
        // Note: actual color output depends on environment
    }

    #[test]
    fn test_colorize_helpers() {
        let tool = colorize_tool_name("read_file");
        assert!(!tool.is_empty());
        
        let path = colorize_file_path("/test/path");
        assert!(!path.is_empty());
    }
}
