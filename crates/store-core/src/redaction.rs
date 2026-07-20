pub fn redact_sensitive(input: &str) -> String {
    let mut output = Vec::new();
    for token in input.split_whitespace() {
        if token.starts_with("ghp_")
            || token.starts_with("github_pat_")
            || token.to_ascii_lowercase().contains("token=")
            || token.to_ascii_lowercase().contains("authorization:")
        {
            output.push("[redacted]");
        } else {
            output.push(token);
        }
    }
    output.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_github_tokens() {
        let line = "GET https://api.github.com?token=secret ghp_123 Authorization: Bearer";
        let redacted = redact_sensitive(line);
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("ghp_123"));
        assert!(redacted.contains("[redacted]"));
    }
}
