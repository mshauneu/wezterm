use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::read_to_string;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use std::{env, fs};

use cached::proc_macro::cached;
use cached::SizedCache;
use config::lua::get_or_create_module;
use config::lua::mlua::{self, Lua};
use luahelper::impl_lua_conversion_dynamic;
use wezterm_dynamic::{FromDynamic, ToDynamic};
use yaml_rust::YamlLoader;

pub fn register(lua: &Lua) -> anyhow::Result<()> {
    let wezterm_mod = get_or_create_module(lua, "wezterm")?;
    wezterm_mod.set("kubernetes_info", lua.create_function(kube_info)?)?;
    Ok(())
}

#[derive(FromDynamic, ToDynamic, Debug, Default, Clone)]
struct KubernetesInfo {
    time: u64,
    context: String,
    user: String,
    namespace: String,
    cluster: String,
}
impl_lua_conversion_dynamic!(KubernetesInfo);

#[derive(FromDynamic, ToDynamic, Debug)]
struct Aliases {
    context: HashMap<String, String>,
    user: HashMap<String, String>,
}
impl_lua_conversion_dynamic!(Aliases);

struct KubeCtxComponents {
    user: Option<String>,
    namespace: Option<String>,
    cluster: Option<String>,
}

fn kube_info<'lua>(_: &'lua Lua, aliases: Aliases) -> mlua::Result<KubernetesInfo> {
    let default_kube_cfg = config::HOME_DIR.to_path_buf().join(".kube").join("config");
    let kube_cfg = get_env("KUBECONFIG").unwrap_or(default_kube_cfg.to_str().unwrap().to_string());

    let time = env::split_paths(&kube_cfg)
        .map(|filename| {
            fs::metadata(filename)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|st| st.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0);

    Ok(get_kube_info(aliases, kube_cfg, time).unwrap_or_else(|| KubernetesInfo::default()))
}

#[cached(
    type = "SizedCache<String, Option<KubernetesInfo>>",
    create = "{ SizedCache::with_size(100) }",
    convert = r#"{ format!("{}-{}", kube_cfg, time) }"#
)]
fn get_kube_info(aliases: Aliases, kube_cfg: String, time: u64) -> Option<KubernetesInfo> {
    let kube_ctx = env::split_paths(&kube_cfg).find_map(get_kube_ctx)?;

    let ctx_components: Vec<Option<KubeCtxComponents>> = env::split_paths(&kube_cfg)
        .map(|filename| get_kube_ctx_component(filename, kube_ctx.clone()))
        .collect();

    let kubernetes_info = KubernetesInfo {
        time,
        user: ctx_components
            .iter()
            .find(|&ctx| match ctx {
                Some(kube) => kube.user.is_some(),
                None => false,
            })
            .and_then(|ctx| {
                ctx.as_ref().map(|kube| {
                    get_kube_user(&aliases.user, kube.user.as_ref().unwrap().as_str()).to_string()
                })
            })
            .unwrap_or(String::default()),

        namespace: ctx_components
            .iter()
            .find(|&ctx| match ctx {
                Some(kube) => kube.namespace.is_some(),
                None => false,
            })
            .and_then(|ctx| ctx.as_ref().map(|kube| kube.namespace.to_owned().unwrap()))
            .unwrap_or(String::default()),

        cluster: ctx_components
            .iter()
            .find(|&ctx| match ctx {
                Some(kube) => kube.cluster.is_some(),
                None => false,
            })
            .and_then(|ctx| ctx.as_ref().map(|kube| kube.cluster.to_owned().unwrap()))
            .unwrap_or(String::default()),

        context: Some(get_kube_ctx_name(&aliases.context, &kube_ctx).to_string())
            .unwrap_or(String::default()),
    };
    Some(kubernetes_info)
}

fn get_kube_ctx(filename: PathBuf) -> Option<String> {
    let contents = read_to_string(filename).ok()?;
    let yaml_docs = YamlLoader::load_from_str(&contents).ok()?;
    if yaml_docs.is_empty() {
        return None;
    }
    let conf = &yaml_docs[0];
    let current_ctx = conf["current-context"].as_str()?;
    if current_ctx.is_empty() {
        return None;
    }
    Some(current_ctx.to_string())
}

fn get_kube_ctx_component(filename: PathBuf, current_ctx: String) -> Option<KubeCtxComponents> {
    let contents = read_to_string(filename).ok()?;

    let yaml_docs = YamlLoader::load_from_str(&contents).ok()?;
    if yaml_docs.is_empty() {
        return None;
    }
    let conf = &yaml_docs[0];

    let ctx_yaml = conf["contexts"].as_vec().and_then(|contexts| {
        contexts
            .iter()
            .filter_map(|ctx| Some((ctx, ctx["name"].as_str()?)))
            .find(|(_, name)| *name == current_ctx)
    });

    let ctx_components = KubeCtxComponents {
        user: ctx_yaml
            .and_then(|(ctx, _)| ctx["context"]["user"].as_str())
            .and_then(|s| {
                if s.is_empty() {
                    return None;
                }
                Some(s.to_owned())
            }),
        namespace: ctx_yaml
            .and_then(|(ctx, _)| ctx["context"]["namespace"].as_str())
            .and_then(|s| {
                if s.is_empty() {
                    return None;
                }
                Some(s.to_owned())
            }),
        cluster: ctx_yaml
            .and_then(|(ctx, _)| ctx["context"]["cluster"].as_str())
            .and_then(|s| {
                if s.is_empty() {
                    return None;
                }
                Some(s.to_owned())
            }),
    };

    Some(ctx_components)
}

fn get_kube_user<'a>(aliases: &'a HashMap<String, String>, kube_user: &'a str) -> Cow<'a, str> {
    get_alias(aliases, kube_user).unwrap_or(Cow::Borrowed(kube_user))
}

fn get_kube_ctx_name<'a>(aliases: &'a HashMap<String, String>, kube_ctx: &'a str) -> Cow<'a, str> {
    get_alias(aliases, kube_ctx).unwrap_or(Cow::Borrowed(kube_ctx))
}

fn get_alias<'a>(
    aliases: &'a HashMap<String, String>,
    alias_candidate: &'a str,
) -> Option<Cow<'a, str>> {
    if let Some(val) = aliases.get(alias_candidate) {
        Some(Cow::Borrowed(val))
    } else {
        aliases.iter().find_map(|(k, v)| {
            let re = regex::Regex::new(&format!("^{}$", k)).ok()?;
            let replaced = re.replace(alias_candidate, &*v);
            match replaced {
                Cow::Owned(replaced) => Some(Cow::Owned(replaced)),
                _ => None,
            }
        })
    }
}

#[inline]
pub fn get_env<K: AsRef<str>>(key: K) -> Option<String> {
    env::var(key.as_ref()).ok()
}
