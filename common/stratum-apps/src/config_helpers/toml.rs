use serde::{
    de::{self, Deserializer},
    Deserialize,
};
use std::{path::PathBuf, time::Duration};

/// Deserialize a duration from a TOML string.
pub fn duration_from_toml<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(serde::Deserialize)]
    struct Helper {
        unit: String,
        value: u64,
    }

    let helper = Helper::deserialize(deserializer)?;
    match helper.unit.as_str() {
        "seconds" => Ok(Duration::from_secs(helper.value)),
        "secs" => Ok(Duration::from_secs(helper.value)),
        "s" => Ok(Duration::from_secs(helper.value)),
        "milliseconds" => Ok(Duration::from_millis(helper.value)),
        "millis" => Ok(Duration::from_millis(helper.value)),
        "ms" => Ok(Duration::from_millis(helper.value)),
        "microseconds" => Ok(Duration::from_micros(helper.value)),
        "micros" => Ok(Duration::from_micros(helper.value)),
        "us" => Ok(Duration::from_micros(helper.value)),
        "nanoseconds" => Ok(Duration::from_nanos(helper.value)),
        "nanos" => Ok(Duration::from_nanos(helper.value)),
        "ns" => Ok(Duration::from_nanos(helper.value)),
        // ... add other units as needed
        _ => Err(serde::de::Error::custom("Unsupported duration unit")),
    }
}

/// Deserialize an optional TOML string into `Option<PathBuf>`, expanding:
/// - `~` (home directory)
/// - environment variables like `$HOME` or `${VAR}`
///
/// Use this for **optional** path fields with:
/// `#[serde(default, deserialize_with = "opt_path_from_toml")]`.
///
/// - Missing field → `None`
/// - Present field → `Some(PathBuf)`
///
/// Fails if the field is present but not a string, or if expansion fails.
pub fn opt_path_from_toml<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;

    opt.map(|raw| {
        let expanded = shellexpand::full(&raw).map_err(|e| de::Error::custom(e.to_string()))?;
        Ok(PathBuf::from(expanded.to_string()))
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ext_config::{Config, File, FileFormat};
    use serde::Deserialize;
    use std::{env, path::PathBuf};

    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default, deserialize_with = "opt_path_from_toml")]
        path: Option<PathBuf>,
    }

    #[test]
    fn missing_field_returns_none() {
        let toml = r#""#;
        let cfg: TestConfig = Config::builder()
            .add_source(File::from_str(toml, FileFormat::Toml))
            .build()
            .and_then(|settings| settings.try_deserialize::<TestConfig>())
            .expect("failed to load a valid toml");
        assert!(cfg.path.is_none());
    }

    #[test]
    fn plain_string_path() {
        let toml = r#"
            path = "/tmp/example"
        "#;

        let cfg: TestConfig = Config::builder()
            .add_source(File::from_str(toml, FileFormat::Toml))
            .build()
            .and_then(|settings| settings.try_deserialize::<TestConfig>())
            .expect("failed to load a valid toml");
        assert_eq!(cfg.path, Some(PathBuf::from("/tmp/example")));
    }

    #[test]
    fn tilde_expands_to_home_directory() {
        let home = env::var("HOME").expect("HOME must be set for this test");

        let toml = r#"
            path = "~/my_app/config"
        "#;

        let cfg: TestConfig = Config::builder()
            .add_source(File::from_str(toml, FileFormat::Toml))
            .build()
            .and_then(|settings| settings.try_deserialize::<TestConfig>())
            .expect("failed to load a valid toml");

        assert_eq!(
            cfg.path,
            Some(PathBuf::from(format!("{home}/my_app/config")))
        );
    }

    #[test]
    fn environment_variable_expansion() {
        env::set_var("TEST_OPT_PATH", "/opt/data");

        let toml = r#"
            path = "$TEST_OPT_PATH/file.txt"
        "#;

        let cfg: TestConfig = Config::builder()
            .add_source(File::from_str(toml, FileFormat::Toml))
            .build()
            .and_then(|settings| settings.try_deserialize::<TestConfig>())
            .expect("failed to load a valid toml");

        assert_eq!(cfg.path, Some(PathBuf::from("/opt/data/file.txt")));
    }

    #[test]
    fn invalid_expansion_fails() {
        let toml = r#"
            path = "$THIS_VAR_DOES_NOT_EXIST/file"
        "#;

        let cfg = Config::builder()
            .add_source(File::from_str(toml, FileFormat::Toml))
            .build()
            .and_then(|settings| settings.try_deserialize::<TestConfig>())
            .unwrap_err();

        assert_eq!(
            "error looking key 'THIS_VAR_DOES_NOT_EXIST' up: environment variable not found",
            cfg.to_string()
        );
    }
}
