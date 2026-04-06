//! Input handling for interactive CLI

use std::io::{self, Write};

/// Detect if input is likely pasted content
pub fn is_pasted_input(input: &str) -> bool {
    // Check for multiple lines
    if input.lines().count() > 2 {
        return true;
    }
    
    // Check for code blocks
    if input.contains("```") || input.contains("fn ") || input.contains("def ") {
        return true;
    }
    
    // Check length
    if input.len() > 200 {
        return true;
    }
    
    false
}

/// Smart newline normalization
pub fn normalize_newlines(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .to_string()
}

/// Read multi-line input
pub fn read_multiline() -> io::Result<String> {
    println!("Entering multi-line mode. Type '.' on a new line to finish.");
    
    let mut lines = Vec::new();
    let stdin = io::stdin();
    
    loop {
        print!("... ");
        io::stdout().flush()?;
        
        let mut line = String::new();
        let bytes = stdin.read_line(&mut line)?;
        
        if bytes == 0 {
            break;
        }
        
        let trimmed = line.trim_end();
        if trimmed == "." || trimmed == ".enter" {
            break;
        }
        
        lines.push(trimmed.to_string());
    }
    
    Ok(lines.join("\n"))
}

/// Input result type
#[derive(Debug, Clone)]
pub enum InputResult {
    Line(String),
    MultiLine(String),
    Empty,
    Eof,
}

/// Read input with smart handling
pub fn read_input(prompt: &str) -> io::Result<InputResult> {
    print!("{}", prompt);
    io::stdout().flush()?;
    
    let mut input = String::new();
    let bytes = io::stdin().read_line(&mut input)?;
    
    if bytes == 0 {
        println!();
        return Ok(InputResult::Eof);
    }
    
    let trimmed = input.trim_end();
    
    // Check for multi-line trigger
    if trimmed == ".multi" || trimmed == ".m" {
        return read_multiline().map(InputResult::MultiLine);
    }
    
    if trimmed.is_empty() {
        return Ok(InputResult::Empty);
    }
    
    // Check if pasted content
    if is_pasted_input(trimmed) {
        let normalized = normalize_newlines(trimmed);
        return Ok(InputResult::MultiLine(normalized));
    }
    
    Ok(InputResult::Line(trimmed.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_paste_detection() {
        assert!(is_pasted_input("fn main() {\n    println!();\n}"));
        assert!(is_pasted_input("```rust\ncode\n```"));
        assert!(!is_pasted_input("Hello"));
    }
    
    #[test]
    fn test_newline_normalization() {
        let input = "Line 1\r\nLine 2\rLine 3  \n";
        let normalized = normalize_newlines(input);
        assert!(!normalized.contains('\r'));
        assert!(!normalized.ends_with("  "));
    }
}
