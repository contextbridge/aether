use super::ImageParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub name: String,
    pub tag: String,
}

impl Image {
    pub fn new(name: impl Into<String>, tag: impl Into<String>) -> Self {
        Self { name: name.into(), tag: tag.into() }
    }

    pub fn parse(reference: &str) -> Result<Self, ImageParseError> {
        if reference.is_empty() || reference.starts_with(':') || reference.ends_with(':') {
            return Err(ImageParseError { reference: reference.to_string() });
        }

        let last_slash = reference.rfind('/');
        let last_colon = reference.rfind(':');
        let image = match last_colon.filter(|colon| last_slash.is_none_or(|slash| *colon > slash)) {
            Some(colon) => Image::new(&reference[..colon], &reference[colon + 1..]),
            None => Image::new(reference, "latest"),
        };
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_image_parse_handles_name_tag() {
        assert_eq!(Image::parse("aether-sandbox:dev").unwrap(), Image::new("aether-sandbox", "dev"));
    }

    #[test]
    fn docker_image_parse_handles_registry_path() {
        assert_eq!(Image::parse("ghcr.io/org/aether:sha").unwrap(), Image::new("ghcr.io/org/aether", "sha"));
    }

    #[test]
    fn docker_image_parse_defaults_latest() {
        assert_eq!(Image::parse("aether-sandbox").unwrap(), Image::new("aether-sandbox", "latest"));
    }

    #[test]
    fn docker_image_parse_rejects_invalid_reference() {
        assert!(Image::parse(":latest").is_err());
        assert!(Image::parse("aether:").is_err());
    }
}
