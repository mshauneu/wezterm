use config::lua::get_or_create_module;
use config::lua::mlua::{self, Lua};
use luahelper::impl_lua_conversion_dynamic;
use wezterm_dynamic::{FromDynamic, ToDynamic};

use ini::Ini;
use std::env;
use std::path::PathBuf;
use std::str::FromStr;

type AwsConfigFile = once_cell::unsync::OnceCell<Option<Ini>>;

#[derive(FromDynamic, ToDynamic)]
struct AwsInfo {
    aws_profile: String,
    aws_region: String,
}
impl_lua_conversion_dynamic!(AwsInfo);

pub fn register(lua: &Lua) -> anyhow::Result<()> {
    let wezterm_mod = get_or_create_module(lua, "wezterm")?;
    wezterm_mod.set("aws_info", lua.create_function(aws_info)?)?;
    Ok(())
}

fn aws_info<'lua>(_: &'lua Lua, _: ()) -> mlua::Result<AwsInfo> {
    let aws_config = AwsConfigFile::new();
    let profile_env_vars = ["AWSU_PROFILE", "AWS_VAULT", "AWSUME_PROFILE", "AWS_PROFILE"];
    let region_env_vars = ["AWS_REGION", "AWS_DEFAULT_REGION"];
    let profile = profile_env_vars.iter().find_map(|env_var| get_env(env_var));
    let region = region_env_vars.iter().find_map(|env_var| get_env(env_var));
    Ok(match (profile, region) {
        (Some(p), Some(r)) => AwsInfo {
            aws_profile: p,
            aws_region: r,
        },
        (None, Some(r)) => AwsInfo {
            aws_profile: "".to_string(),
            aws_region: r,
        },
        (Some(p), None) => AwsInfo {
            aws_profile: p.clone(),
            aws_region: opt_string(get_aws_region_from_config(&Some(p), &aws_config)),
        },
        (None, None) => AwsInfo {
            aws_profile: "".to_string(),
            aws_region: opt_string(get_aws_region_from_config(&None, &aws_config)),
        },
    })
}

fn opt_string(s: Option<String>) -> String {
    match s {
        Some(s) => s,
        None => "".to_string(),
    }
}

fn get_aws_region_from_config(
    aws_profile: &Option<String>,
    aws_config: &AwsConfigFile,
) -> Option<String> {
    let config = get_config(aws_config)?;
    let section = get_profile_config(config, aws_profile)?;
    section.get("region").map(std::borrow::ToOwned::to_owned)
}

fn get_profile_config<'a>(
    config: &'a Ini,
    profile: &Option<String>,
) -> Option<&'a ini::Properties> {
    match profile {
        Some(profile) => config.section(Some(format!("profile {}", profile))),
        None => config.section(Some("default")),
    }
}

fn get_config<'a>(config: &'a AwsConfigFile) -> Option<&'a Ini> {
    config
        .get_or_init(|| {
            let path = get_config_file_path()?;
            Ini::load_from_file(path).ok()
        })
        .as_ref()
}

fn get_config_file_path() -> Option<PathBuf> {
    get_env("AWS_CONFIG_FILE")
        .and_then(|path| PathBuf::from_str(&path).ok())
        .or_else(|| {
            let mut home = config::HOME_DIR.to_path_buf();
            home.push(".aws/config");
            Some(home)
        })
}

#[inline]
pub fn get_env<K: AsRef<str>>(key: K) -> Option<String> {
    env::var(key.as_ref()).ok()
}
