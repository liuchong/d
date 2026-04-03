//! Read-Eval-Print Loop for interactive chat

use std::io::{self, Write};

/// REPL for interactive chat
pub struct Repl {
    history: Vec<String>,
    history_index: Option<usize>,
}

impl Repl {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            history_index: None,
        }
    }

    /// Read a line from stdin with basic history support
    pub fn read_line(&mut self, prompt: &str) -> io::Result<Option<String>> {
        print!("{}", prompt);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let input = input.trim().to_string();
        
        if input.is_empty() {
            return Ok(None);
        }

        // Add to history
        if self.history.is_empty() || self.history.last().unwrap() != &input {
            self.history.push(input.clone());
        }
        self.history_index = None;

        Ok(Some(input))
    }

    /// Get previous history entry
    pub fn previous_history(&mut self) -> Option<&str> {
        if self.history.is_empty() {
            return None;
        }

        let idx = match self.history_index {
            None => self.history.len() - 1,
            Some(i) if i > 0 => i - 1,
            Some(i) => i,
        };
        
        self.history_index = Some(idx);
        self.history.get(idx).map(|s| s.as_str())
    }

    /// Get next history entry
    pub fn next_history(&mut self) -> Option<&str> {
        match self.history_index {
            None => None,
            Some(i) if i + 1 < self.history.len() => {
                self.history_index = Some(i + 1);
                self.history.get(i + 1).map(|s| s.as_str())
            }
            _ => {
                self.history_index = None;
                None
            }
        }
    }
}

impl Default for Repl {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repl_history() {
        let mut repl = Repl::new();
        
        repl.history.push("first".to_string());
        repl.history.push("second".to_string());
        repl.history.push("third".to_string());
        
        // Test previous
        assert_eq!(repl.previous_history(), Some("third"));
        assert_eq!(repl.previous_history(), Some("second"));
        assert_eq!(repl.previous_history(), Some("first"));
        assert_eq!(repl.previous_history(), Some("first")); // Stays at first
        
        // Test next
        assert_eq!(repl.next_history(), Some("second"));
        assert_eq!(repl.next_history(), Some("third"));
        assert_eq!(repl.next_history(), None); // Past end
    }
}
