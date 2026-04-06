//! Validation utilities

use std::net::IpAddr;

/// Email validation (basic)
pub fn is_valid_email(email: &str) -> bool {
    // Simple regex-like validation
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return false;
    }
    
    let local = parts[0];
    let domain = parts[1];
    
    // Local part checks
    if local.is_empty() || local.len() > 64 {
        return false;
    }
    
    // Domain checks
    if domain.is_empty() || domain.len() > 255 {
        return false;
    }
    
    if !domain.contains('.') {
        return false;
    }
    
    true
}

/// URL validation
pub fn is_valid_url(url: &str) -> bool {
    url::Url::parse(url).is_ok()
}

/// IP address validation
pub fn is_valid_ip(ip: &str) -> bool {
    ip.parse::<IpAddr>().is_ok()
}

/// IPv4 validation
pub fn is_valid_ipv4(ip: &str) -> bool {
    use std::net::Ipv4Addr;
    ip.parse::<Ipv4Addr>().is_ok()
}

/// IPv6 validation
pub fn is_valid_ipv6(ip: &str) -> bool {
    use std::net::Ipv6Addr;
    ip.parse::<Ipv6Addr>().is_ok()
}

/// Port validation
pub fn is_valid_port(port: u16) -> bool {
    port > 0
}

/// Semver validation (simplified)
pub fn is_valid_semver(version: &str) -> bool {
    // Basic semver: MAJOR.MINOR.PATCH
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    
    parts.iter().all(|p| p.parse::<u64>().is_ok())
}

/// Hex color validation
pub fn is_valid_hex_color(color: &str) -> bool {
    let color = color.trim_start_matches('#');
    
    if color.len() != 3 && color.len() != 6 {
        return false;
    }
    
    color.chars().all(|c| c.is_ascii_hexdigit())
}

/// UUID validation
pub fn is_valid_uuid(uuid: &str) -> bool {
    let uuid = uuid.replace("-", "");
    
    if uuid.len() != 32 {
        return false;
    }
    
    uuid.chars().all(|c| c.is_ascii_hexdigit())
}

/// Slug validation (URL-friendly string)
pub fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 100 {
        return false;
    }
    
    slug.chars().all(|c| {
        c.is_ascii_lowercase() ||
        c.is_ascii_digit() ||
        c == '-' ||
        c == '_'
    })
}

/// Check if string is numeric
pub fn is_numeric(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}

/// Check if string is integer
pub fn is_integer(s: &str) -> bool {
    s.parse::<i64>().is_ok()
}

/// Check if string is boolean
pub fn is_boolean(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "true" | "false" | "1" | "0" | "yes" | "no")
}

/// Password strength check (basic)
pub fn password_strength(password: &str) -> PasswordStrength {
    let length = password.len();
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());
    
    let score = [
        length >= 8,
        length >= 12,
        has_upper,
        has_lower,
        has_digit,
        has_special,
    ]
    .iter()
    .filter(|&&x| x)
    .count();
    
    match score {
        0..=2 => PasswordStrength::Weak,
        3..=4 => PasswordStrength::Medium,
        5..=6 => PasswordStrength::Strong,
        _ => PasswordStrength::Weak,
    }
}

/// Password strength levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordStrength {
    Weak,
    Medium,
    Strong,
}

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

impl ValidationResult {
    /// Create valid result
    pub fn valid() -> Self {
        Self {
            valid: true,
            errors: vec![],
        }
    }

    /// Create invalid result
    pub fn invalid(error: impl Into<String>) -> Self {
        Self {
            valid: false,
            errors: vec![error.into()],
        }
    }

    /// Add error
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.errors.push(error.into());
        self.valid = false;
        self
    }

    /// Combine with another result
    pub fn combine(mut self, other: ValidationResult) -> Self {
        self.errors.extend(other.errors);
        if !other.valid {
            self.valid = false;
        }
        self
    }
}

/// Validator trait
pub trait Validator<T> {
    /// Validate value
    fn validate(&self, value: &T) -> ValidationResult;
}

/// Required validator
pub struct Required;

impl<T> Validator<Option<T>> for Required {
    fn validate(&self, value: &Option<T>) -> ValidationResult {
        match value {
            Some(_) => ValidationResult::valid(),
            None => ValidationResult::invalid("Value is required"),
        }
    }
}

/// Length validator
pub struct Length {
    min: usize,
    max: usize,
}

impl Length {
    /// Create length validator
    pub fn new(min: usize, max: usize) -> Self {
        Self { min, max }
    }
}

impl Validator<String> for Length {
    fn validate(&self, value: &String) -> ValidationResult {
        let len = value.len();
        if len < self.min {
            ValidationResult::invalid(format!("Too short (min {})", self.min))
        } else if len > self.max {
            ValidationResult::invalid(format!("Too long (max {})", self.max))
        } else {
            ValidationResult::valid()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_validation() {
        assert!(is_valid_email("test@example.com"));
        assert!(!is_valid_email("invalid"));
        assert!(!is_valid_email("@example.com"));
    }

    #[test]
    fn test_url_validation() {
        assert!(is_valid_url("https://example.com"));
        assert!(is_valid_url("http://localhost:3000"));
        assert!(!is_valid_url("not a url"));
    }

    #[test]
    fn test_uuid_validation() {
        assert!(is_valid_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_uuid("550e8400e29b41d4a716446655440000"));
        assert!(!is_valid_uuid("not-a-uuid"));
    }

    #[test]
    fn test_hex_color() {
        assert!(is_valid_hex_color("#ff0000"));
        assert!(is_valid_hex_color("#f00"));
        assert!(!is_valid_hex_color("#gg0000"));
    }

    #[test]
    fn test_password_strength() {
        assert_eq!(password_strength("abc"), PasswordStrength::Weak);
        assert_eq!(password_strength("Abc123!@#"), PasswordStrength::Strong);
    }

    #[test]
    fn test_length_validator() {
        let validator = Length::new(3, 10);
        assert!(validator.validate(&"hello".to_string()).valid);
        assert!(!validator.validate(&"hi".to_string()).valid);
        assert!(!validator.validate(&"hello world".to_string()).valid);
    }
}
