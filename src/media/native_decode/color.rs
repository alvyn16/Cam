//! Preserve encoded color tags when Windows omits them from its media type.
use anyhow::{Result, bail, ensure};

pub(super) fn description(
    stream: &serde_json::Value,
    matrix: u32,
    full_range: bool,
) -> Result<(u32, bool)> {
    let matrix = match stream["color_space"].as_str() {
        Some("bt709") => 1,
        Some("smpte170m" | "bt470bg") => 2,
        Some("unknown") | None => matrix,
        Some(other) => bail!("native preview color matrix {other} is unsupported; use FFmpeg"),
    };
    ensure!(
        matches!(matrix, 1 | 2),
        "native preview color matrix is unsupported; use FFmpeg"
    );
    let full_range = match stream["color_range"].as_str() {
        Some("pc") => true,
        Some("tv") => false,
        _ => full_range,
    };
    Ok((matrix, full_range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encoded_tags_override_missing_or_incorrect_windows_defaults() {
        assert_eq!(
            description(&json!({"color_space":"bt709","color_range":"pc"}), 2, false).unwrap(),
            (1, true)
        );
        assert_eq!(
            description(
                &json!({"color_space":"bt470bg","color_range":"tv"}),
                1,
                true
            )
            .unwrap(),
            (2, false)
        );
        assert_eq!(
            description(&json!({"color_space":"smpte170m"}), 1, true).unwrap(),
            (2, true)
        );
        assert_eq!(description(&json!({}), 2, false).unwrap(), (2, false));
        assert_eq!(
            description(
                &json!({"color_space":"unknown","color_range":"unknown"}),
                1,
                true
            )
            .unwrap(),
            (1, true)
        );
    }

    #[test]
    fn unsupported_matrices_require_the_compatibility_decoder() {
        for matrix in ["bt2020nc", "bt2020c", "fcc", "gbr"] {
            assert!(description(&json!({"color_space":matrix}), 2, false).is_err());
        }
        assert!(description(&json!({}), 9, false).is_err());
    }
}
