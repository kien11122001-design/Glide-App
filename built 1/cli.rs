//! Command-line parsing (FR-23). Pure, so it is unit-tested on any OS.

use crate::config::DEFAULT_CONFIG_TOML;
use std::path::PathBuf;

pub const HELP: &str = "\
Glide - smooth scrolling for Windows mouse wheels

USAGE:
    glide [OPTIONS]

OPTIONS:
    --config <path>          Use this config file instead of %APPDATA%\\Glide\\config.toml
    --disable                Start with smoothing disabled (toggle from the tray)
    --print-default-config   Print the commented default config and exit
    --version                Print the version and exit
    -h, --help               Print this help and exit
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub config: Option<PathBuf>,
    pub disable: bool,
    pub print_default_config: bool,
    pub version: bool,
    pub help: bool,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => match it.next() {
                Some(p) if !p.starts_with("--") => out.config = Some(PathBuf::from(p)),
                _ => return Err("--config needs a file path".to_owned()),
            },
            "--disable" => out.disable = true,
            "--print-default-config" => out.print_default_config = true,
            "--version" | "-V" => out.version = true,
            "--help" | "-h" => out.help = true,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(out)
}

/// Text for the informational flags, if one was given (then the app just exits).
pub fn info_text(args: &Args) -> Option<String> {
    if args.help {
        Some(HELP.to_owned())
    } else if args.version {
        Some(format!("glide {}", env!("CARGO_PKG_VERSION")))
    } else if args.print_default_config {
        Some(DEFAULT_CONFIG_TOML.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn parses_flags() {
        assert_eq!(p(&[]), Ok(Args::default()));
        let a = p(&["--config", "C:\\x.toml", "--disable"]).unwrap();
        assert_eq!(a.config, Some(PathBuf::from("C:\\x.toml")));
        assert!(a.disable && !a.version);
        assert!(p(&["--version"]).unwrap().version);
        assert!(p(&["-h"]).unwrap().help);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--config"]).is_err());
        assert!(p(&["--config", "--disable"]).is_err());
        assert!(p(&["--nope"]).is_err());
    }

    #[test]
    fn info_flags_produce_text() {
        assert!(info_text(&Args::default()).is_none());
        let v = info_text(&p(&["--version"]).unwrap()).unwrap();
        assert!(v.starts_with("glide "));
        let cfg = info_text(&p(&["--print-default-config"]).unwrap()).unwrap();
        assert_eq!(cfg, DEFAULT_CONFIG_TOML);
        assert!(info_text(&p(&["--help"]).unwrap()).unwrap().contains("--config"));
    }
}
