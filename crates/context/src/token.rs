//! Token estimation for content

use llm::Message;

/// Estimate token count from text
/// 
/// Uses a heuristic based on:
/// - Character count (adjusted for ASCII vs non-ASCII ratio)
/// - Word count
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut ascii_count = 0;
    let mut byte_count = 0;
    let mut word_count = 0;
    let mut in_word = false;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if !in_word {
                word_count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }

        if ch.is_ascii() {
            ascii_count += 1;
        }
        byte_count += ch.len_utf8();
    }

    // Calculate ratio
    let ratio = if byte_count > 0 {
        ascii_count as f32 / byte_count as f32
    } else {
        1.0
    };

    // Estimate based on content type
    let char_estimate = if ratio > 0.9 {
        // Mostly ASCII - ~4 chars per token
        text.len() / 4
    } else if ratio > 0.5 {
        // Mixed - ~3 chars per token
        text.len() / 3
    } else {
        // Mostly non-ASCII - ~1.5 chars per token
        text.len() * 2 / 3
    };

    // Average with word-based estimate (~0.75 words per token on average)
    let word_estimate = word_count;

    std::cmp::max(1, (char_estimate + word_estimate) / 2)
}

/// Estimate tokens for a message
pub fn estimate_message_tokens(msg: &Message) -> usize {
    let mut count = 0;
    count += estimate_tokens(&msg.content);
    
    // Add overhead for message structure
    count += 10;
    
    count
}

/// Estimate total tokens for messages
pub fn estimate_messages_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_ascii() {
        let text = "Hello, World!";
        let tokens = estimate_tokens(text);
        assert!(tokens > 0);
        assert!(tokens <= 10);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_cjk() {
        let text = "你好世界";
        let tokens = estimate_tokens(text);
        // CJK characters are typically 1-2 tokens each
        assert!(tokens >= 2);
    }

    #[test]
    fn test_estimate_tokens_code() {
        let code = r#"fn main() { println!("hi"); }"#;
        let tokens = estimate_tokens(code);
        assert!(tokens > 3);
    }

    #[test]
    fn test_estimate_tokens_long_ascii() {
        let text = "a".repeat(1000);
        let tokens = estimate_tokens(&text);
        // 1000 chars with word-based adjustment
        assert!(tokens > 100);
        assert!(tokens < 500);
    }

    #[test]
    fn test_estimate_message_tokens() {
        let msg = Message::user("Hello world this is a test");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 10); // Content + overhead
    }

    #[test]
    fn test_estimate_messages_tokens_empty() {
        let messages: Vec<Message> = vec![];
        assert_eq!(estimate_messages_tokens(&messages), 0);
    }

    #[test]
    fn test_estimate_messages_tokens_multiple() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there"),
        ];
        let tokens = estimate_messages_tokens(&messages);
        assert!(tokens > 20); // Two messages with overhead
    }
}
