#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.ai.model, "kimi-k2-5");
        assert_eq!(config.ai.base_url, "https://api.moonshot.cn/v1");
        assert_eq!(config.ai.temperature, 0.7);
        assert_eq!(config.server.host, "localhost");
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn test_ai_config_default() {
        let ai = AiConfig::default();
        assert!(ai.api_key.is_empty());
        assert_eq!(ai.max_tokens, 32768);
    }

    #[test]
    fn test_server_config_default() {
        let server = ServerConfig::default();
        assert_eq!(server.port, 8080);
        assert_eq!(server.root, std::path::PathBuf::from("."));
    }
}
