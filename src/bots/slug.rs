#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SlugError {
    #[error("directory name is empty after slugifying")]
    Empty,
    #[error("directory slug is longer than 63 bytes")]
    TooLong,
}

pub fn slugify(dir: &str) -> Result<String, SlugError> {
    let mut slug = String::with_capacity(dir.len());
    for ch in dir.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() || matches!(lower, '.' | '_' | '-')
        {
            slug.push(lower);
        } else {
            slug.push('-');
        }
    }

    let slug = slug.trim_matches(|ch| matches!(ch, '.' | '_' | '-'));
    if slug.is_empty() {
        return Err(SlugError::Empty);
    }
    if slug.len() > 63 {
        return Err(SlugError::TooLong);
    }
    Ok(slug.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_replaces_unsafe_characters() {
        assert_eq!(slugify("Echo Bot").unwrap(), "echo-bot");
        assert_eq!(slugify("Echo_Bot.v2").unwrap(), "echo_bot.v2");
        assert_eq!(slugify("a/b:c").unwrap(), "a-b-c");
        assert_eq!(slugify("Ünicode").unwrap(), "nicode");
    }

    #[test]
    fn trims_leading_and_trailing_separators() {
        assert_eq!(slugify("--Echo__Bot--").unwrap(), "echo__bot");
        assert!(slugify("...").is_err());
    }

    #[test]
    fn rejects_empty_and_overlong_slugs() {
        assert!(slugify("").is_err());
        assert!(slugify("---").is_err());
        assert!(slugify(&"a".repeat(63)).is_ok());
        assert!(slugify(&"a".repeat(64)).is_err());
    }
}
