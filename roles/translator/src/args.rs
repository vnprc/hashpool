use std::path::PathBuf;

#[derive(Debug)]
pub struct Args {
    pub config_path: PathBuf,
    pub global_config_path: PathBuf,
}

enum ArgsState {
    Next,
    ExpectConfigPath,
    ExpectGlobalPath,
    Done,
}

enum ArgsResult {
    Config(PathBuf),
    Global(PathBuf),
    None,
    Help(String),
}

impl Args {
    const DEFAULT_CONFIG_PATH: &'static str = "proxy-config.toml";
    const HELP_MSG: &'static str = "Usage: -h/--help, -c/--config <path|default proxy-config.toml>, -g/--global <path>";

    pub fn from_args() -> Result<Self, String> {
        let cli_args: Vec<String> = std::env::args().skip(1).collect();

        if cli_args.is_empty() {
            println!("Using default config path: {}", Self::DEFAULT_CONFIG_PATH);
            println!("{}\n", Self::HELP_MSG);
        }

        let mut config_path: Option<PathBuf> = None;
        let mut global_config_path: Option<PathBuf> = None;

        let results: Vec<_> = cli_args
            .into_iter()
            .scan(ArgsState::Next, |state, item| {
                match *state {
                    ArgsState::Next => match item.as_str() {
                        "-c" | "--config" => {
                            *state = ArgsState::ExpectConfigPath;
                            Some(ArgsResult::None)
                        }
                        "-g" | "--global" => {
                            *state = ArgsState::ExpectGlobalPath;
                            Some(ArgsResult::None)
                        }
                        "-h" | "--help" => Some(ArgsResult::Help(Self::HELP_MSG.to_string())),
                        _ => {
                            *state = ArgsState::Next;
                            Some(ArgsResult::None)
                        }
                    },
                    ArgsState::ExpectConfigPath => {
                        let path = PathBuf::from(item.clone());
                        if !path.exists() {
                            return Some(ArgsResult::Help(format!(
                                "Error: File '{}' does not exist!",
                                path.display()
                            )));
                        }
                        *state = ArgsState::Next;
                        Some(ArgsResult::Config(path))
                    }
                    ArgsState::ExpectGlobalPath => {
                        let path = PathBuf::from(item.clone());
                        if !path.exists() {
                            return Some(ArgsResult::Help(format!(
                                "Error: File '{}' does not exist!",
                                path.display()
                            )));
                        }
                        *state = ArgsState::Next;
                        Some(ArgsResult::Global(path))
                    }
                    ArgsState::Done => None,
                }
            })
            .collect();

        for res in results {
            match res {
                ArgsResult::Config(p) => config_path = Some(p),
                ArgsResult::Global(p) => global_config_path = Some(p),
                ArgsResult::Help(h) => return Err(h),
                _ => {}
            }
        }

        let config_path = config_path.unwrap_or_else(|| PathBuf::from(Self::DEFAULT_CONFIG_PATH));
        let global_config_path = global_config_path.ok_or("Missing -g/--global <path>")?;

        Ok(Self { config_path, global_config_path })
    }
}
