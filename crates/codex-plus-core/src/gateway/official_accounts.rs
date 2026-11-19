//! 网关只保存本地账号库引用，token 的读取与轮换仍由原账号库负责。
use anyhow::{Context, ensure};
use serde_json::{Value, json};

use crate::official_accounts::OfficialAccountStore;
use crate::settings::{RelayMode, RelayProfile, RelayProtocol};

pub const ACCOUNT_REF: &str = "codex_plus_official_account_ref";
pub const SCHEDULE_PREFIX: &str = "local-official-schedule-";

/// 旧会话 provider/model 到当前网关模型的精确迁移表。来源必须仍在绑定的池中，顺序组保持官方专用规则。
pub fn legacy_provider_routes(
    gateway: &super::store::GatewaySettings,
) -> serde_json::Map<String, Value> {
    use crate::tools::ToolId;
    let mut aliases = serde_json::Map::new();
    let Some(binding) = gateway.active_binding(&ToolId::Codex) else {
        return aliases;
    };
    let exposed = super::route::codex_pool_models(gateway, binding);
    for relay in super::route::binding_pool_relays(gateway, binding) {
        let Some(account) = account_ref(relay) else {
            continue;
        };
        let provider = crate::official_experience::direct_official_provider_id(&account);
        for model in super::route::relay_models(relay) {
            let qualified = format!("{}/{model}", relay.id);
            let target = if exposed.contains(&qualified) {
                qualified
            } else if exposed.contains(&model)
                && super::route::serving_count(
                    &super::route::binding_pool_relays(gateway, binding),
                    &model,
                ) == 1
            {
                model.clone()
            } else {
                continue;
            };
            aliases.insert(format!("{provider}:{model}"), json!(target));
        }
    }
    for group in &gateway.groups {
        if !group.id.starts_with(SCHEDULE_PREFIX) {
            continue;
        }
        let Ok(plan) = super::route::resolve_plan(gateway, &ToolId::Codex, &group.model_id(), None)
        else {
            continue;
        };
        if !schedule_plan(&plan) || validate_schedule(gateway, &plan).is_err() {
            continue;
        }
        let model = &plan.candidates[0].upstream_model;
        let key = format!("{}:{model}", crate::official_account_schedule::PROVIDER_ID);
        // 多个同模型但账号次序不同的组不能擅自挑一个。
        match aliases.get(&key) {
            None => {
                aliases.insert(key, json!(group.model_id()));
            }
            Some(_) => {
                aliases.insert(key, Value::Null);
            }
        }
    }
    aliases.retain(|_, value| value.is_string());
    aliases
}

pub fn current_schedule_links() -> anyhow::Result<Vec<RelayProfile>> {
    let settings = crate::settings::SettingsStore::default().load()?;
    let accounts = OfficialAccountStore::default();
    let mut relays = Vec::new();
    for id in &settings.official_experience.account_schedule_order {
        let models = crate::official_model_catalog::cached_official_model_entries_for_account(id)
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| {
                entry
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        relays.push(linked_relay(&accounts, id, &models)?);
    }
    ensure!(!relays.is_empty(), "当前没有官方顺序调度账号");
    Ok(relays)
}

/// 命令行接入前只准备池和组，不启动 GUI、不切换配置；在活跃网关上拒绝自动迁移。
pub fn migrate_inactive_pool() -> anyhow::Result<usize> {
    let relays = current_schedule_links()?;
    let store = super::store::GatewayStore::default();
    ensure!(
        !store.load()?.any_enabled(),
        "网关正在使用，请通过界面的显式迁移入口接入"
    );
    let path = super::store::GatewayStore::default_path();
    if path.exists() {
        let backup = path.with_extension(format!(
            "json.bak.{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S-%f")
        ));
        ensure!(!backup.exists(), "迁移备份路径已存在");
        std::fs::copy(&path, backup)?;
    }
    let (_, added) = store.update(|gateway| {
        ensure!(!gateway.any_enabled(), "网关状态已改变，停止迁移");
        merge_links(gateway, &relays, None)
    })?;
    Ok(added)
}

/// 只新增引用与明确的官方顺序组，不接管 Agent、不改变默认模型，也不删除旧配置。
pub fn merge_links(
    gateway: &mut super::store::GatewaySettings,
    relays: &[RelayProfile],
    skip_agents: Option<&[String]>,
) -> anyhow::Result<usize> {
    use super::store::{GatewayGroup, GatewayProvider, GroupMember, GroupMode, GroupSticky};
    use sha2::{Digest, Sha256};
    let mut added = 0;
    let mut models = std::collections::BTreeMap::<String, Vec<GroupMember>>::new();
    for relay in relays {
        let id = account_ref(relay).context("迁移只接受账号库引用")?;
        let source = format!("official-account:{id}");
        if !gateway.providers.iter().any(|entry| entry.source == source) {
            let provider: GatewayProvider =
                serde_json::from_value(json!({"enabled":true, "source":source, "relay":relay}))?;
            gateway.providers.push(provider);
            for (agent, binding) in &mut gateway.bindings {
                let excluded =
                    skip_agents.map_or(!binding.auto_join_providers, |skip| skip.contains(agent));
                if excluded && !binding.pool_exclude.contains(&relay.id) {
                    binding.pool_exclude.push(relay.id.clone());
                }
            }
            added += 1;
        }
        let provider = gateway
            .providers
            .iter()
            .find(|entry| entry.source == source)
            .unwrap();
        ensure!(
            account_ref(&provider.relay).as_deref() == Some(&id),
            "已存在来源不是本地账号引用，停止迁移"
        );
        for model in super::route::relay_models(&provider.relay) {
            models.entry(model.clone()).or_default().push(GroupMember {
                model: format!("{}/{model}", provider.relay.id),
                weight: 1,
            });
        }
    }
    for (model, members) in models {
        let digest = format!("{:x}", Sha256::digest(model.as_bytes()));
        let id = format!("{SCHEDULE_PREFIX}{}", &digest[..16]);
        // 已迁移的组不覆盖用户后续编辑；有意同步顺序需要显式处理。
        if gateway.groups.iter().any(|group| group.id == id) {
            continue;
        }
        gateway.groups.push(GatewayGroup {
            id,
            name: format!("官方顺序调度 · {model}"),
            mode: GroupMode::InOrder,
            sticky: GroupSticky::Off,
            members,
            ..Default::default()
        });
    }
    Ok(added)
}

pub(crate) fn schedule_plan(plan: &super::route::RoutePlan) -> bool {
    plan.group_id
        .as_deref()
        .is_some_and(|id| id.starts_with(SCHEDULE_PREFIX))
        && !plan.candidates.is_empty()
        && plan.candidates.iter().all(|candidate| {
            is_reference(&candidate.relay)
                && crate::official_login::member_uses_official_login_transport(&candidate.relay)
                && candidate.upstream_model == plan.candidates[0].upstream_model
        })
}

pub(crate) fn validate_schedule(
    gateway: &super::store::GatewaySettings,
    plan: &super::route::RoutePlan,
) -> anyhow::Result<()> {
    if let Some(id) = plan
        .group_id
        .as_deref()
        .filter(|id| id.starts_with(SCHEDULE_PREFIX))
    {
        let group = gateway.group(id).context("官方顺序组不存在")?;
        ensure!(
            group.mode == super::store::GroupMode::InOrder
                && group.sticky == super::store::GroupSticky::Off,
            "官方顺序组不允许权重、轮转或会话重新排序"
        );
        ensure!(
            plan.candidates.is_empty() || schedule_plan(plan),
            "官方顺序组不得混入第三方、其他模型或独立登录快照"
        );
        for member in &group.members {
            let (provider, model) = member
                .model
                .split_once('/')
                .context("官方顺序组只能使用明确账号/模型")?;
            let provider = gateway.provider(provider).context("官方顺序组账号已删除")?;
            ensure!(
                is_reference(&provider.relay) && member.weight == 1 && !model.contains('*'),
                "官方顺序组成员无效"
            );
        }
    }
    Ok(())
}

pub fn account_ref(relay: &RelayProfile) -> Option<String> {
    let value: Value = serde_json::from_str(&relay.auth_contents).ok()?;
    value.get(ACCOUNT_REF)?.as_str().map(str::to_string)
}

pub fn is_reference(relay: &RelayProfile) -> bool {
    serde_json::from_str::<Value>(&relay.auth_contents)
        .ok()
        .is_some_and(|value| value.get(ACCOUNT_REF).is_some())
}

pub fn linked_relay(
    store: &OfficialAccountStore,
    account_id: &str,
    models: &[String],
) -> anyhow::Result<RelayProfile> {
    let account = store.get(account_id)?;
    ensure!(
        account.enabled && account.status == "ready",
        "官方账号未就绪"
    );
    ensure!(
        !models.is_empty(),
        "该账号尚无已确认模型目录，请先刷新官方目录"
    );
    Ok(RelayProfile {
        id: format!("local-official-{account_id}"),
        name: account.name,
        relay_mode: RelayMode::Official,
        protocol: RelayProtocol::Responses,
        model: models[0].clone(),
        model_list: models.join("\n"),
        auth_contents: json!({ACCOUNT_REF: account_id, "codex_plus_gateway": true}).to_string(),
        ..RelayProfile::default()
    })
}

/// 引用必须先于快照解析处理；损坏或已删除的引用不得退回 live 登录或网关快照。
pub async fn credential(
    relay: &RelayProfile,
    store: &OfficialAccountStore,
    force: bool,
) -> anyhow::Result<crate::official_login::OfficialCredential> {
    ensure!(
        relay.relay_mode == RelayMode::Official
            && relay.protocol == RelayProtocol::Responses
            && !relay.official_mix_api_key
            && relay.base_url.trim().is_empty()
            && relay.api_key.trim().is_empty(),
        "本地官方账号引用不允许混入 API 地址或密钥"
    );
    let id = account_ref(relay).context("本地官方账号引用格式无效")?;
    ensure!(
        !crate::official_login::is_lent_to_codex(relay),
        "旧借用标记不能用账号库引用绕过，请先确认独立登录归属"
    );
    ensure!(!id.trim().is_empty(), "本地官方账号引用为空");
    let account = store.get(&id)?;
    ensure!(
        account.enabled && account.status == "ready",
        "所引用官方账号不可用"
    );
    store.refresh_tokens(&id, force).await?;
    let auth = store.get_auth_json(&id)?;
    let parsed = crate::official_accounts::parse_official_auth(&auth)?;
    ensure!(parsed.id == id, "官方账号凭据身份不匹配");
    Ok(crate::official_login::OfficialCredential {
        account_scope: crate::native_auth::account_scope(&auth.to_string()),
        access_token: crate::official_accounts::auth_token(&auth, "access_token")
            .context("所引用官方账号缺少访问令牌")?
            .to_string(),
        account_id: if parsed.chatgpt_account_id.is_empty() {
            parsed.workspace_id
        } else {
            parsed.chatgpt_account_id
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn legacy_routes_keep_exact_accounts_and_refuse_ambiguous_or_mixed_schedules() {
        use super::super::store::{AgentBinding, GatewayProvider, GatewaySettings};
        let mut gateway = GatewaySettings::default();
        let relays: Vec<_> = ["one", "two"]
            .iter()
            .map(|id| RelayProfile {
                id: format!("official-{id}"),
                relay_mode: RelayMode::Official,
                protocol: RelayProtocol::Responses,
                model: "gpt-catalog-model".into(),
                model_list: "gpt-catalog-model".into(),
                auth_contents: json!({ACCOUNT_REF:id, "codex_plus_gateway":true}).to_string(),
                ..Default::default()
            })
            .collect();
        merge_links(&mut gateway, &relays, None).unwrap();
        gateway.bindings.insert(
            "codex".into(),
            AgentBinding {
                enabled: true,
                ..Default::default()
            },
        );
        let key = "codex_plus_official_schedule:gpt-catalog-model";
        let account_key = format!(
            "{}:gpt-catalog-model",
            crate::official_experience::direct_official_provider_id("one")
        );
        let aliases = legacy_provider_routes(&gateway);
        assert_eq!(aliases[&account_key], "official-one/gpt-catalog-model");
        assert_eq!(aliases[key], gateway.groups[0].model_id());
        let mut duplicate = gateway.groups[0].clone();
        duplicate.id.push_str("-other");
        gateway.groups.push(duplicate);
        assert!(!legacy_provider_routes(&gateway).contains_key(key));
        gateway.groups.pop();
        gateway.providers.push(serde_json::from_value::<GatewayProvider>(json!({"enabled":true,
            "relay": RelayProfile { id: "api".into(), relay_mode: RelayMode::PureApi,
                model: "gpt-catalog-model".into(), model_list: "gpt-catalog-model".into(), ..Default::default() }
        })).unwrap());
        gateway.groups[0].members[0].model = "api/gpt-catalog-model".into();
        let aliases = legacy_provider_routes(&gateway);
        assert!(!aliases.contains_key(key));
        assert_eq!(aliases[&account_key], "official-one/gpt-catalog-model");
        gateway
            .bindings
            .get_mut("codex")
            .unwrap()
            .pool_exclude
            .push("official-one".into());
        assert!(!legacy_provider_routes(&gateway).contains_key(&account_key));
    }

    #[test]
    fn migration_is_idempotent_and_preserves_pool_bindings_and_account_order() {
        let relays: Vec<_> = ["two", "one"]
            .iter()
            .map(|id| RelayProfile {
                id: format!("official-{id}"),
                name: format!("Account {id}"),
                relay_mode: RelayMode::Official,
                protocol: RelayProtocol::Responses,
                model: "gpt-from-cache".into(),
                model_list: "gpt-from-cache".into(),
                auth_contents: json!({ACCOUNT_REF:id, "codex_plus_gateway":true}).to_string(),
                ..Default::default()
            })
            .collect();
        let mut gateway = super::super::store::GatewaySettings::default();
        let before = gateway.bindings.clone();
        assert_eq!(merge_links(&mut gateway, &relays, None).unwrap(), 2);
        assert_eq!(merge_links(&mut gateway, &relays, None).unwrap(), 0);
        assert_eq!(gateway.bindings, before);
        assert_eq!(gateway.providers.len(), 2);
        assert_eq!(gateway.groups.len(), 1);
        let temp = tempfile::tempdir().unwrap();
        let store = super::super::store::GatewayStore::at(temp.path().join("gateway.json"));
        store.save(&gateway).unwrap();
        let reloaded = store.load().unwrap();
        assert_eq!(
            account_ref(&reloaded.providers[0].relay).as_deref(),
            Some("two")
        );
        assert!(crate::official_login::member_uses_official_login_transport(
            &reloaded.providers[0].relay
        ));
        assert_eq!(
            gateway.groups[0].members[0].model,
            "official-two/gpt-from-cache"
        );
        assert_eq!(
            gateway.groups[0].members[1].model,
            "official-one/gpt-from-cache"
        );
        assert!(
            !serde_json::to_string(&gateway)
                .unwrap()
                .contains("refresh_token")
        );
        gateway.bindings.insert(
            "codex".into(),
            super::super::store::AgentBinding {
                enabled: true,
                ..Default::default()
            },
        );
        let plan = super::super::route::resolve_plan(
            &gateway,
            &crate::tools::ToolId::Codex,
            &gateway.groups[0].model_id(),
            None,
        )
        .unwrap();
        assert!(schedule_plan(&plan));
        validate_schedule(&gateway, &plan).unwrap();
        gateway.groups[0].mode = super::super::store::GroupMode::Weighted;
        assert!(validate_schedule(&gateway, &plan).is_err());
        gateway.groups[0].mode = super::super::store::GroupMode::InOrder;
        gateway.groups[0].members[0].weight = 2;
        assert!(validate_schedule(&gateway, &plan).is_err());
    }

    fn jwt(value: Value) -> String {
        format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&value).unwrap())
        )
    }

    #[tokio::test]
    async fn linking_keeps_only_a_reference_and_reads_the_same_account_without_copying_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let metadata = temp.path().join("accounts.json");
        let secrets = temp.path().join("secrets.json");
        let store = OfficialAccountStore::new(metadata.clone(), secrets.clone());
        let token = jwt(
            json!({"sub":"one", "exp":4102444800u64, "https://api.openai.com/auth":{"chatgpt_account_id":"workspace-one"}}),
        );
        let (account, _) = store.upsert_auth_json(json!({"auth_mode":"chatgpt", "tokens":{"access_token": token, "id_token": token, "refresh_token":"keep-refresh", "account_id":"workspace-one"}})).unwrap();
        let before = std::fs::read(&secrets).unwrap();
        let relay =
            linked_relay(&store, &account.id, &["model-from-account-catalog".into()]).unwrap();
        assert!(!relay.auth_contents.contains("keep-refresh"));
        assert!(!relay.auth_contents.contains(&token));
        assert!(crate::official_login::is_gateway_owned_login(&relay));
        let resolved = credential(&relay, &store, false).await.unwrap();
        assert_eq!(resolved.access_token, token);
        assert_eq!(resolved.account_id, "workspace-one");
        assert_eq!(std::fs::read(&secrets).unwrap(), before);
        store
            .update(
                &account.id,
                crate::official_accounts::OfficialAccountPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(credential(&relay, &store, false).await.is_err());
    }

    #[tokio::test]
    async fn deleted_reference_never_uses_embedded_or_live_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let store = OfficialAccountStore::new(
            temp.path().join("accounts.json"),
            temp.path().join("secrets.json"),
        );
        let relay = RelayProfile {
            relay_mode: RelayMode::Official,
            protocol: RelayProtocol::Responses,
            auth_contents: json!({ACCOUNT_REF: "deleted", "tokens": {"access_token": "untrusted"}})
                .to_string(),
            ..RelayProfile::default()
        };
        assert!(crate::official_login::member_uses_official_login_transport(
            &relay
        ));
        assert!(credential(&relay, &store, false).await.is_err());
        assert!(!temp.path().join("accounts.json").exists());
    }

    #[test]
    fn empty_reference_and_api_profiles_are_not_usable_credentials() {
        let relay = RelayProfile {
            auth_contents: json!({ACCOUNT_REF: ""}).to_string(),
            ..RelayProfile::default()
        };
        assert!(is_reference(&relay));
        assert!(
            !crate::official_login::member_uses_official_login_transport(&RelayProfile {
                base_url: "https://example.com".into(),
                api_key: "key".into(),
                ..relay
            })
        );
        assert!(is_reference(&RelayProfile {
            auth_contents: json!({ACCOUNT_REF: 123}).to_string(),
            ..RelayProfile::default()
        }));
    }
}
