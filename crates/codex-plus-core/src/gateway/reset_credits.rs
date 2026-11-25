//! ChatGPT 订阅账号的「额度重置卡」：认得出、看得见、按确认用掉一张。
//!
//! `wham/usage` 给的是 5 小时 / 每周窗口的用量百分比；重置卡是另一份数据
//! （`wham/rate-limit-reset-credits`）：账号手上有几张、什么时候到期、能不能用。
//! 消费走同一前缀的 `/consume`，只要一个幂等请求标识，服务端自己挑一张可用卡。
//!
//! 消费是不可撤销的扣减：调用方必须先拿到用户的明确确认，且同一次操作只能用同一个
//! 请求标识重试——换标识重发等于再扣一张。
//!
//! 只在内存缓存；重启后重新查。读不到就当作「不知道」，不显示成 0 张。

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::settings::RelayProfile;

const CHATGPT_RESET_CREDITS_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
/// 消费地址就是查询地址加 `/consume`。
const CHATGPT_RESET_CONSUME_SUFFIX: &str = "/consume";
/// 只认这一类卡：其它重置类型（例如别的产品的额度）不属于 Codex 窗口。
const RESET_TYPE: &str = "codex_rate_limits";
const MAX_CREDITS: usize = 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 一张重置卡。`usable` 是这里算出来的：上游给了 `status`，但过期与否要按当前时间判。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCredit {
    pub id: String,
    /// `available` / `redeemed` / `expired` / `redeeming` / `pending` / `unknown`。
    pub status: String,
    /// 发到手的时间（毫秒时间戳）。
    pub granted_at_ms: Option<u64>,
    /// 到期时间（毫秒时间戳）；没有到期时间表示不会过期。
    pub expires_at_ms: Option<u64>,
    /// 现在就能用：状态是 `available` 且没过期。
    pub usable: bool,
}

/// 一个账号手上的全部重置卡，外加列表接口给的汇总数。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditsView {
    /// 上游汇总的可用张数；列表接口没给就为空，别拿 `credits` 里数出来的顶替。
    pub available_count: Option<u64>,
    /// 当前套餐下能用的张数。
    pub applicable_available_count: Option<u64>,
    /// 累计获得过的张数。
    pub total_earned_count: Option<u64>,
    pub credits: Vec<ResetCredit>,
    /// 这次读到的时间（毫秒时间戳）。
    pub observed_at_ms: u64,
}

impl ResetCreditsView {
    /// 现在能用的张数：优先用逐张判定的结果，没有明细时才退回汇总数。
    pub fn usable_count(&self) -> Option<u64> {
        self.usable_count_at(now_ms())
    }

    fn usable_count_at(&self, now: u64) -> Option<u64> {
        if self.credits.is_empty() {
            return self.applicable_available_count.or(self.available_count);
        }
        Some(
            self.credits
                .iter()
                .filter(|credit| credit.usable && credit.expires_at_ms.is_none_or(|at| at > now))
                .count() as u64,
        )
    }
}

/// 消费的结果。`Unknown` 必须按「可能已扣、也可能没扣」处理：让用户保留同一请求标识，
/// 不要另建一次操作。其余都是确定结果，未确认操作随之结束。
///
/// code 与官方 codex `ConsumeRateLimitResetCreditCode` 一致：
/// `reset` / `nothing_to_reset` / `no_credit` / `already_redeemed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeOutcome {
    /// 这次真的重置了。
    Reset,
    /// 同一请求标识之前已经重置过，这次没有重复扣。
    AlreadyRedeemed,
    /// 账号没有可用的重置卡，没有扣卡。
    NoCredit,
    /// 当前额度不需要重置，没有扣卡。
    NothingToReset,
    /// 服务端明确拒绝（4xx，认证失败与 409 除外），没有扣卡；带 HTTP 状态码。
    Rejected(u16),
    /// 服务端返回了别的 code：不能断言成功，也不能断言失败。
    Unknown,
}

impl ConsumeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::AlreadyRedeemed => "alreadyRedeemed",
            Self::NoCredit => "noCredit",
            Self::NothingToReset => "nothingToReset",
            Self::Rejected(_) => "rejected",
            Self::Unknown => "unknown",
        }
    }

    /// 确定结果：这次操作已经结束，不必（也不应）再用同一标识重试。
    pub fn settled(self) -> bool {
        self != Self::Unknown
    }

    /// 卡确实用在了这次操作上（本次或此前同一标识）。
    pub fn redeemed(self) -> bool {
        matches!(self, Self::Reset | Self::AlreadyRedeemed)
    }
}

static CACHE: LazyLock<Mutex<HashMap<String, ResetCreditsView>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, ResetCreditsView>> {
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 读缓存；没查过返回 `None`（界面上不显示成 0 张）。
pub fn cached(relay_id: &str) -> Option<ResetCreditsView> {
    let mut view = lock().get(relay_id).cloned()?;
    let now = now_ms();
    for credit in &mut view.credits {
        credit.usable &= credit.expires_at_ms.is_none_or(|at| at > now);
    }
    Some(view)
}

/// 只落盘操作标识，不保存登录凭据。按官方账号隔离，使同账号的多个供应商引用共用一次操作。
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RedemptionJournal {
    pending: Option<String>,
    completed: Vec<String>,
    /// 已结束、被放弃的标识，或损坏记录修复时生成的本地版本标识。
    #[serde(default)]
    settled: Vec<String>,
}

/// 每份记录最多留多少个已结束的标识。
const JOURNAL_HISTORY: usize = 64;

impl RedemptionJournal {
    fn valid(&self) -> bool {
        self.pending.as_deref().is_none_or(valid_request_id)
            && self.completed.len() <= JOURNAL_HISTORY
            && self.completed.iter().all(|id| valid_request_id(id))
            && self.settled.len() <= JOURNAL_HISTORY
            && self.settled.iter().all(|id| valid_request_id(id))
    }
}

fn remember(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|value| value == id) {
        list.push(id.to_owned());
    }
    if list.len() > JOURNAL_HISTORY {
        list.drain(..list.len() - JOURNAL_HISTORY);
    }
}

/// 拿到账号的独占锁，返回锁、记录路径和读记录文件的原始结果（解析由调用方决定）。
fn lock_journal(
    root: &Path,
    account_id: &str,
) -> anyhow::Result<(File, PathBuf, std::io::Result<Vec<u8>>)> {
    anyhow::ensure!(
        !account_id.trim().is_empty(),
        "缺少官方账号标识，未发送重置请求"
    );
    fs::create_dir_all(root)?;
    let name = format!("{:x}", Sha256::digest(account_id.as_bytes()));
    let path = root.join(format!("{name}.json"));
    // 锁稳定的独立文件；不等待另一个联网请求，避免阻塞异步运行时。
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(format!("{name}.lock")))?;
    FileExt::try_lock_exclusive(&file).context("该账号已有重置操作正在处理，请稍后重试")?;
    let bytes = fs::read(&path);
    Ok((file, path, bytes))
}

fn save_journal(path: &Path, journal: &RedemptionJournal) -> anyhow::Result<()> {
    crate::settings::atomic_write(path, &serde_json::to_vec(journal)?)
        .context("无法保存重置操作记录，未确认本次结果")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationStatus {
    Ready,
    Pending,
    Damaged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationPreview {
    pub status: OperationStatus,
    pub confirmation: String,
}

/// 预览的标识不暴露账号或请求 ID。执行时持同一把账号锁重新比较，旧确认不能应用于新记录。
fn read_operation(
    account_id: &str,
    bytes: std::io::Result<Vec<u8>>,
) -> anyhow::Result<(OperationPreview, Option<RedemptionJournal>)> {
    let mut digest = Sha256::new();
    digest.update(Sha256::digest(account_id.as_bytes()));
    let journal = match bytes {
        Ok(bytes) => {
            digest.update([1]);
            digest.update(&bytes);
            serde_json::from_slice::<RedemptionJournal>(&bytes)
                .ok()
                .filter(RedemptionJournal::valid)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            digest.update([0]);
            Some(RedemptionJournal::default())
        }
        Err(error) => return Err(error.into()),
    };
    let status = match &journal {
        Some(journal) if journal.pending.is_some() => OperationStatus::Pending,
        Some(_) => OperationStatus::Ready,
        None => OperationStatus::Damaged,
    };
    Ok((
        OperationPreview {
            status,
            confirmation: format!("{:x}", digest.finalize()),
        },
        journal,
    ))
}

fn require_confirmation(preview: &OperationPreview, confirmation: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        preview.confirmation == confirmation,
        "重置操作已变化，未执行；请核实当前状态后重新确认"
    );
    Ok(())
}

fn preview_in(root: &Path, account_id: &str) -> anyhow::Result<OperationPreview> {
    let (_lock, _, bytes) = lock_journal(root, account_id)?;
    Ok(read_operation(account_id, bytes)?.0)
}

fn abandon_in(root: &Path, account_id: &str, confirmation: &str) -> anyhow::Result<()> {
    let (_lock, path, bytes) = lock_journal(root, account_id)?;
    let (preview, journal) = read_operation(account_id, bytes)?;
    require_confirmation(&preview, confirmation)?;
    anyhow::ensure!(
        preview.status != OperationStatus::Ready,
        "没有未确认的重置操作"
    );
    let mut journal = journal.unwrap_or_else(|| {
        // 两次修复不能回到同一份空记录，否则更早的确认可能再次生效。
        let mut repaired = RedemptionJournal::default();
        repaired.settled.push(new_request_id());
        repaired
    });
    if let Some(pending) = journal.pending.take() {
        remember(&mut journal.settled, &pending);
    }
    save_journal(&path, &journal)?;
    forget_reset_start(&path);
    Ok(())
}

struct Redemption {
    _lock: File,
    path: PathBuf,
    journal: RedemptionJournal,
    request_id: String,
    fresh: bool,
}

impl Redemption {
    fn open(root: &Path, account_id: &str, confirmation: &str) -> anyhow::Result<Self> {
        let (file, path, bytes) = lock_journal(root, account_id)?;
        let (preview, journal) = read_operation(account_id, bytes)?;
        require_confirmation(&preview, confirmation)?;
        let journal = journal.context("重置操作记录损坏，未发送请求")?;
        let fresh = journal.pending.is_none();
        let request_id = journal.pending.clone().unwrap_or_else(new_request_id);
        let mut operation = Self {
            _lock: file,
            path,
            journal,
            request_id,
            fresh,
        };
        if fresh {
            operation.journal.pending = Some(operation.request_id.clone());
            operation.save()?; // 持久成功后才允许 POST；重启/切页后复用未确认操作。
        }
        Ok(operation)
    }

    fn save(&self) -> anyhow::Result<()> {
        save_journal(&self.path, &self.journal)
    }

    /// 记下确定结果并结束未确认操作；`Unknown` 保留同一标识等下次重试。
    fn finish(&mut self, outcome: ConsumeOutcome) -> anyhow::Result<()> {
        if !outcome.settled() {
            return Ok(());
        }
        if outcome.redeemed() {
            remember(&mut self.journal.completed, &self.request_id);
        } else {
            remember(&mut self.journal.settled, &self.request_id);
        }
        self.journal.pending = None;
        self.save()
    }
}

struct ResetStart {
    request_id: String,
    relay_id: String,
    started_at: Instant,
}

// 额度冷却也只存在于进程内，重启后不猜测旧操作对应哪个冷却。
static RESET_STARTS: LazyLock<Mutex<HashMap<PathBuf, ResetStart>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn forget_reset_start(path: &Path) -> Option<ResetStart> {
    RESET_STARTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(path)
}

fn record_reset_start(operation: &Redemption, relay_id: &str, started_at: Instant) {
    if !operation.fresh {
        return;
    }
    let mut starts = RESET_STARTS.lock().unwrap_or_else(|e| e.into_inner());
    if starts.len() >= 256
        && let Some(oldest) = starts
            .iter()
            .min_by_key(|(_, value)| value.started_at)
            .map(|(path, _)| path.clone())
    {
        starts.remove(&oldest);
    }
    starts.insert(
        operation.path.clone(),
        ResetStart {
            request_id: operation.request_id.clone(),
            relay_id: relay_id.to_owned(),
            started_at,
        },
    );
}

fn finish_reset(operation: &mut Redemption, outcome: ConsumeOutcome) -> anyhow::Result<()> {
    if !operation.fresh
        && let ConsumeOutcome::Rejected(status) = outcome
    {
        anyhow::bail!("核实请求被拒绝（HTTP {status}），上次重置结果仍未知；请稍后重试");
    }
    if !outcome.settled() {
        return Ok(());
    }
    operation.finish(outcome)?;
    if let Some(start) = forget_reset_start(&operation.path)
        && start.request_id == operation.request_id
        && outcome.redeemed()
    {
        super::key_pool::clear_official_quota_cooldown(&start.relay_id, start.started_at);
    }
    Ok(())
}

pub fn record(relay_id: &str, view: ResetCreditsView) {
    lock().insert(relay_id.to_string(), view);
}

pub fn forget(relay_id: &str) {
    lock().remove(relay_id);
}

/// 秒级时间戳或 ISO 时间串转毫秒；不合法返回 `None`。
fn epoch_ms(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(seconds) = value.as_f64() {
        if seconds > 0.0 {
            return Some((seconds * 1000.0) as u64);
        }
        return None;
    }
    let text = value.as_str()?;
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp_millis().max(0) as u64)
}

fn count(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}

/// 解析 `wham/rate-limit-reset-credits`。
///
/// 结构不认识就返回 `None`——宁可显示「查不到」，也不要把有卡显示成没卡。
pub fn parse_credits(value: &Value, now: u64) -> Option<ResetCreditsView> {
    let object = value.as_object()?;
    let raw = object.get("credits")?;
    // 没有 `credits` 数组说明接口换了形状；空数组若没有汇总数，仍保持未知。
    let list = raw.as_array()?;
    if list.len() > MAX_CREDITS {
        return None;
    }
    let credits = list
        .iter()
        .filter_map(|item| {
            let item = item.as_object()?;
            if item.get("reset_type")?.as_str()? != RESET_TYPE {
                return None;
            }
            let id = item.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() || id.len() > 256 {
                return None;
            }
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .filter(|status| {
                    matches!(
                        *status,
                        "available" | "redeemed" | "expired" | "redeeming" | "pending"
                    )
                })
                .unwrap_or("unknown")
                .to_string();
            let expires_at_ms = epoch_ms(item.get("expires_at"));
            let expiry_passed = expires_at_ms.is_some_and(|at| at <= now);
            Some(ResetCredit {
                id,
                usable: status == "available" && !expiry_passed,
                status,
                granted_at_ms: epoch_ms(item.get("granted_at")),
                expires_at_ms,
            })
        })
        .collect();
    Some(ResetCreditsView {
        available_count: count(object.get("available_count")),
        applicable_available_count: count(object.get("applicable_available_count")),
        total_earned_count: count(object.get("total_earned_count")),
        credits,
        observed_at_ms: now,
    })
}

/// 是不是能查重置卡的账号：只有 ChatGPT 订阅（官方登录传输）有。
pub fn supported(relay: &RelayProfile) -> bool {
    crate::official_login::member_uses_official_login_transport(relay)
}

fn client_for(relay: &RelayProfile) -> anyhow::Result<reqwest::Client> {
    crate::http_client::build_for_relay(reqwest::Client::builder().timeout(REQUEST_TIMEOUT), relay)
}

async fn fetch(relay: &RelayProfile) -> anyhow::Result<ResetCreditsView> {
    if !supported(relay) {
        anyhow::bail!("只有 ChatGPT 订阅账号能查重置卡");
    }
    let credential = crate::official_login::resolve_official_credential_for_quota(relay).await?;
    let mut request = client_for(relay)?
        .get(CHATGPT_RESET_CREDITS_URL)
        .bearer_auth(&credential.access_token)
        .header(reqwest::header::ACCEPT, "application/json");
    if !credential.account_id.is_empty() {
        request = request.header("chatgpt-account-id", credential.account_id.as_str());
    }
    let response = request.send().await?;
    let status = response.status();
    if status.as_u16() == 404 {
        anyhow::bail!("这个账号不支持重置卡");
    }
    if !status.is_success() {
        anyhow::bail!("查询重置卡失败：HTTP {}", status.as_u16());
    }
    let now = now_ms();
    parse_credits(&response.json::<Value>().await?, now)
        .ok_or_else(|| anyhow::anyhow!("重置卡响应格式不认识，保留上次读数"))
}

/// 查一次并写缓存。查不到时保留上次读数，由调用方决定怎么提示。
pub async fn refresh(relay: &RelayProfile) -> anyhow::Result<ResetCreditsView> {
    let view = fetch(relay).await?;
    record(&relay.id, view.clone());
    Ok(view)
}

/// 缓存超过 `max_age_ms` 才真的去查，用来挡界面的重复刷新。
pub async fn refresh_if_stale(
    relay: &RelayProfile,
    max_age_ms: u64,
) -> anyhow::Result<Option<ResetCreditsView>> {
    let now = now_ms();
    if let Some(view) = cached(&relay.id)
        && now.saturating_sub(view.observed_at_ms) < max_age_ms
    {
        return Ok(Some(view));
    }
    refresh(relay).await.map(Some)
}

/// 请求标识只要稳定、可重放就够了：非空、不太长、没有控制字符。
pub fn valid_request_id(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed == value
        && !trimmed.chars().any(|ch| ch.is_control())
}

fn operations_dir() -> PathBuf {
    crate::paths::default_app_state_dir().join("reset-credit-operations")
}

/// 用掉一张重置卡。
///
/// 确认绑定到已预览的账号记录。网络中断后，只能重试已保存的同一标识。
pub async fn consume(relay: &RelayProfile, confirmation: &str) -> anyhow::Result<ConsumeOutcome> {
    if !supported(relay) {
        anyhow::bail!("只有 ChatGPT 订阅账号能使用重置卡");
    }
    let credential = crate::official_login::resolve_official_credential_for_quota(relay).await?;
    let client = client_for(relay)?;
    let mut operation = Redemption::open(&operations_dir(), &credential.account_id, confirmation)?;
    let url = format!("{CHATGPT_RESET_CREDITS_URL}{CHATGPT_RESET_CONSUME_SUFFIX}");
    record_reset_start(&operation, &relay.id, Instant::now());
    let outcome = send_redemption(&client, &url, &credential, &operation.request_id).await?;
    // 收到服务端响应后清缓存：下次读到的才是服务端的真实状态。
    forget(&relay.id);
    finish_reset(&mut operation, outcome)?;
    Ok(outcome)
}

async fn send_redemption(
    client: &reqwest::Client,
    url: &str,
    credential: &crate::official_login::OfficialCredential,
    request_id: &str,
) -> anyhow::Result<ConsumeOutcome> {
    let mut request = client
        .post(url)
        .bearer_auth(&credential.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({ "redeem_request_id": request_id }));
    if !credential.account_id.is_empty() {
        request = request.header("chatgpt-account-id", credential.account_id.as_str());
    }
    // 超时、连接断开：请求可能已到服务端，结果未知，保留同一标识重试。
    let response = request.send().await.map_err(|error| {
        let reason = if error.is_timeout() {
            "请求超时"
        } else if error.is_connect() {
            "连接失败"
        } else {
            "网络中断"
        };
        anyhow::anyhow!("{reason}，重置结果未知，请稍后用同一操作重试：{error}")
    })?;
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        anyhow::bail!("登录凭据无效，请恢复登录后重试同一操作");
    }
    if !(200..300).contains(&status) {
        // 其余 4xx 是服务端明确拒绝、没有处理这次请求；409 可能是同一标识正在处理，
        // 和 5xx、其它异常状态一样按结果未知保留。
        if (400..500).contains(&status) && status != 409 {
            return Ok(ConsumeOutcome::Rejected(status));
        }
        anyhow::bail!("服务端返回 HTTP {status}，重置结果未知，请稍后用同一操作重试");
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    Ok(outcome_of_code(body.get("code").and_then(Value::as_str)))
}

/// 官方 `ConsumeRateLimitResetCreditCode` 的四种 code；认不出的按结果未知处理。
fn outcome_of_code(code: Option<&str>) -> ConsumeOutcome {
    match code {
        Some("reset") => ConsumeOutcome::Reset,
        Some("already_redeemed") => ConsumeOutcome::AlreadyRedeemed,
        Some("no_credit") => ConsumeOutcome::NoCredit,
        Some("nothing_to_reset") => ConsumeOutcome::NothingToReset,
        _ => ConsumeOutcome::Unknown,
    }
}

pub async fn preview_operation(relay: &RelayProfile) -> anyhow::Result<OperationPreview> {
    if !supported(relay) {
        anyhow::bail!("只有 ChatGPT 订阅账号能使用重置卡");
    }
    let credential = crate::official_login::resolve_official_credential_for_quota(relay).await?;
    preview_in(&operations_dir(), &credential.account_id)
}

/// 仅放弃已确认的那笔操作，或修复已确认的那份损坏记录。
pub async fn abandon_pending(relay: &RelayProfile, confirmation: &str) -> anyhow::Result<()> {
    if !supported(relay) {
        anyhow::bail!("只有 ChatGPT 订阅账号能使用重置卡");
    }
    let credential = crate::official_login::resolve_official_credential_for_quota(relay).await?;
    abandon_in(&operations_dir(), &credential.account_id, confirmation)
}

/// 生成一个可以重放的请求标识（UUID v4）。
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(id: &str, status: &str, expires_at: Option<i64>) -> Value {
        json!({
            "id": id,
            "reset_type": "codex_rate_limits",
            "status": status,
            "granted_at": 1_700_000_000,
            "expires_at": expires_at,
        })
    }

    #[test]
    fn parses_available_and_expired_cards() {
        let now = 1_700_000_000_000;
        let value = json!({
            "credits": [
                card("a", "available", Some(1_700_000_500)),
                card("b", "available", Some(1_600_000_000)),
                card("c", "redeemed", Some(1_700_000_500)),
                { "id": "d", "reset_type": "other_product", "status": "available" },
                { "id": "e", "reset_type": "codex_rate_limits", "status": "available", "expires_at": 1_700_000_500 },
            ],
            "available_count": 3,
            "applicable_available_count": 2,
            "total_earned_count": 5,
        });
        let view = parse_credits(&value, now).unwrap();
        // 只留 codex_rate_limits（d 是别的产品，丢掉）；过期的 available 不算可用。
        assert_eq!(view.credits.len(), 4);
        assert_eq!(view.credits[0].id, "a");
        assert!(view.credits[0].usable);
        assert_eq!(view.credits[1].id, "b");
        assert!(view.credits[1].expires_at_ms.is_some());
        assert!(!view.credits[1].usable);
        assert_eq!(view.credits[2].id, "c");
        assert!(!view.credits[2].usable);
        assert_eq!(view.credits[3].id, "e");
        assert!(view.credits[3].usable);
        assert_eq!(view.available_count, Some(3));
        assert_eq!(view.applicable_available_count, Some(2));
        assert_eq!(view.total_earned_count, Some(5));
        assert_eq!(view.usable_count_at(now), Some(2));
    }

    #[test]
    fn empty_list_without_summary_remains_unknown() {
        let view = parse_credits(&json!({ "credits": [] }), 1).unwrap();
        assert!(view.credits.is_empty());
        assert_eq!(view.available_count, None);
        assert_eq!(view.usable_count(), None);
    }

    #[test]
    fn unknown_shape_is_none() {
        assert!(parse_credits(&json!({ "items": [] }), 1).is_none());
        assert!(parse_credits(&json!([]), 1).is_none());
    }

    #[test]
    fn request_id_rules() {
        assert!(valid_request_id("2b1f3a4c-0000-4000-8000-000000000000"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("  x  "));
        assert!(!valid_request_id("bad\nid"));
        assert!(!valid_request_id(&"x".repeat(200)));
        assert!(valid_request_id(&new_request_id()));
    }

    #[test]
    fn cached_card_availability_expires_at_the_boundary() {
        let view =
            parse_credits(&json!({ "credits": [card("a", "available", Some(2))] }), 1).unwrap();
        assert_eq!(view.usable_count_at(1999), Some(1));
        assert_eq!(view.usable_count_at(2000), Some(0));
    }

    fn current(root: &Path, account_id: &str) -> OperationPreview {
        preview_in(root, account_id).unwrap()
    }

    fn open_current(root: &Path, account_id: &str) -> Redemption {
        Redemption::open(root, account_id, &current(root, account_id).confirmation).unwrap()
    }

    #[test]
    fn ready_confirmation_cannot_retry_and_pending_confirmation_cannot_start() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let ready = current(root, "account-a");
        assert_eq!(ready.status, OperationStatus::Ready);
        let first = Redemption::open(root, "account-a", &ready.confirmation).unwrap();
        let first_id = first.request_id.clone();
        drop(first);
        assert!(Redemption::open(root, "account-a", &ready.confirmation).is_err());
        let pending = current(root, "account-a");
        assert_eq!(pending.status, OperationStatus::Pending);
        let mut retry = Redemption::open(root, "account-a", &pending.confirmation).unwrap();
        assert!(!retry.fresh);
        assert_eq!(retry.request_id, first_id);
        retry.finish(ConsumeOutcome::Reset).unwrap();
        drop(retry);
        assert!(Redemption::open(root, "account-a", &pending.confirmation).is_err());
        assert!(Redemption::open(root, "account-a", &ready.confirmation).is_err());
        let next = open_current(root, "account-a");
        assert!(next.fresh);
        assert_ne!(next.request_id, first_id);
    }

    #[test]
    fn redemptions_serialize_per_account_and_isolate_accounts() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let a = current(root, "account-a").confirmation;
        let b = current(root, "account-b").confirmation;
        assert_ne!(a, b);
        assert!(Redemption::open(root, "account-a", &b).is_err());
        let first = Redemption::open(root, "account-a", &a).unwrap();
        assert!(preview_in(root, "account-a").is_err());
        assert!(Redemption::open(root, "account-a", &a).is_err());
        let other = Redemption::open(root, "account-b", &b).unwrap();
        assert_ne!(first.request_id, other.request_id);
        drop(first);
    }

    #[test]
    fn corrupt_redemption_journal_does_not_silently_start_a_new_operation() {
        let root = tempfile::tempdir().unwrap();
        let first = open_current(root.path(), "account-a");
        let path = first.path.clone();
        drop(first);
        fs::write(path, b"broken").unwrap();
        let preview = current(root.path(), "account-a");
        assert_eq!(preview.status, OperationStatus::Damaged);
        assert!(Redemption::open(root.path(), "account-a", &preview.confirmation).is_err());
        assert!(preview_in(root.path(), "").is_err());
    }

    /// 测试用的一次性服务端：每个连接读完一个请求，记下 `redeem_request_id`，按顺序回应。
    /// `None` 表示读完请求后直接断开连接，不给响应。
    fn serve(replies: Vec<Option<(u16, Value)>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/consume", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut ids = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 2048];
                loop {
                    let count = stream.read(&mut chunk).unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(boundary) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..boundary]).to_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:").map(str::trim))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if bytes.len() >= boundary + 4 + length {
                            let body: Value =
                                serde_json::from_slice(&bytes[boundary + 4..boundary + 4 + length])
                                    .unwrap();
                            ids.push(body["redeem_request_id"].as_str().unwrap().to_string());
                            break;
                        }
                    }
                }
                let Some((status, body)) = reply else {
                    drop(stream);
                    continue;
                };
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
            ids
        });
        (url, server)
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    fn test_credential() -> crate::official_login::OfficialCredential {
        crate::official_login::OfficialCredential {
            account_scope: None,
            access_token: "test-only".into(),
            account_id: "test-account".into(),
        }
    }

    #[tokio::test]
    async fn unknown_http_outcome_reuses_the_saved_id_on_retry() {
        use super::super::key_pool::{self, FailureClass, KeyRef, OFFICIAL_KEY_ID};

        let (url, server) = serve(vec![
            None,
            Some((429, json!({ "error": "slow down" }))),
            Some((200, json!({ "code": "already_redeemed" }))),
        ]);
        let root = tempfile::tempdir().unwrap();
        let client = test_client();
        let credential = test_credential();
        let official = |relay_id: &str| KeyRef {
            relay_id: relay_id.into(),
            key_id: OFFICIAL_KEY_ID.into(),
            fingerprint: 0,
        };
        let original = official("reset-operation-original");
        let alias = official("reset-operation-alias");
        let now = Instant::now();
        let exhausted = FailureClass::QuotaExhausted { reset_after: None };
        key_pool::record_failure(&original, "", &exhausted, now);
        key_pool::record_failure(&alias, "", &exhausted, now);
        let first = open_current(root.path(), &credential.account_id);
        let request_id = first.request_id.clone();
        record_reset_start(&first, &original.relay_id, now);
        assert!(
            send_redemption(&client, &url, &credential, &request_id)
                .await
                .is_err()
        );
        drop(first);
        assert!(key_pool::cooling_remaining(&original, "", now).is_some());
        let pending = current(root.path(), &credential.account_id);
        let mut limited =
            Redemption::open(root.path(), &credential.account_id, &pending.confirmation).unwrap();
        let outcome = send_redemption(&client, &url, &credential, &limited.request_id)
            .await
            .unwrap();
        assert_eq!(outcome, ConsumeOutcome::Rejected(429));
        assert!(finish_reset(&mut limited, outcome).is_err());
        drop(limited);
        assert!(key_pool::cooling_remaining(&original, "", now).is_some());
        let mut retry =
            Redemption::open(root.path(), &credential.account_id, &pending.confirmation).unwrap();
        record_reset_start(&retry, &alias.relay_id, now + Duration::from_secs(1));
        let outcome = send_redemption(&client, &url, &credential, &retry.request_id)
            .await
            .unwrap();
        assert_eq!(outcome, ConsumeOutcome::AlreadyRedeemed);
        finish_reset(&mut retry, outcome).unwrap();
        drop(retry);
        assert!(key_pool::cooling_remaining(&original, "", now).is_none());
        assert!(key_pool::cooling_remaining(&alias, "", now).is_some());
        assert!(
            Redemption::open(root.path(), &credential.account_id, &pending.confirmation).is_err()
        );
        assert_eq!(server.join().unwrap(), vec![request_id; 3]);
        key_pool::reset(&alias.relay_id);
    }

    #[test]
    fn retry_cannot_clear_a_newer_cooldown_or_one_without_an_original_start() {
        use super::super::key_pool::{self, FailureClass, KeyRef, OFFICIAL_KEY_ID};

        let root = tempfile::tempdir().unwrap();
        for has_start in [true, false] {
            let account = if has_start { "present" } else { "missing" };
            let key = KeyRef {
                relay_id: format!("reset-operation-start-{account}"),
                key_id: OFFICIAL_KEY_ID.into(),
                fingerprint: 0,
            };
            let started_at = Instant::now();
            let first = open_current(root.path(), account);
            if has_start {
                record_reset_start(&first, &key.relay_id, started_at);
            }
            drop(first);
            let later = started_at + Duration::from_secs(1);
            key_pool::record_failure(
                &key,
                "",
                &FailureClass::QuotaExhausted { reset_after: None },
                later,
            );
            let mut retry = open_current(root.path(), account);
            record_reset_start(&retry, &key.relay_id, later);
            finish_reset(&mut retry, ConsumeOutcome::AlreadyRedeemed).unwrap();
            assert!(key_pool::cooling_remaining(&key, "", later).is_some());
            key_pool::reset(&key.relay_id);
        }
    }

    #[test]
    fn official_consume_codes_are_all_recognized() {
        assert_eq!(outcome_of_code(Some("reset")), ConsumeOutcome::Reset);
        assert_eq!(
            outcome_of_code(Some("already_redeemed")),
            ConsumeOutcome::AlreadyRedeemed
        );
        assert_eq!(outcome_of_code(Some("no_credit")), ConsumeOutcome::NoCredit);
        assert_eq!(
            outcome_of_code(Some("nothing_to_reset")),
            ConsumeOutcome::NothingToReset
        );
        assert_eq!(outcome_of_code(Some("other")), ConsumeOutcome::Unknown);
        assert_eq!(outcome_of_code(None), ConsumeOutcome::Unknown);
    }

    /// 没扣卡的确定结果（无卡、无需重置、4xx 拒绝）清掉未确认操作，且旧标识不再发出去。
    #[tokio::test]
    async fn settled_outcomes_without_redemption_clear_pending() {
        let (url, server) = serve(vec![
            Some((200, json!({ "code": "no_credit" }))),
            Some((200, json!({ "code": "nothing_to_reset" }))),
            Some((400, json!({ "error": "bad request" }))),
            Some((429, json!({ "error": "slow down" }))),
        ]);
        let root = tempfile::tempdir().unwrap();
        let client = test_client();
        let credential = test_credential();
        let mut expected = Vec::new();
        for want in [
            ConsumeOutcome::NoCredit,
            ConsumeOutcome::NothingToReset,
            ConsumeOutcome::Rejected(400),
            ConsumeOutcome::Rejected(429),
        ] {
            let ready = current(root.path(), &credential.account_id);
            let mut operation =
                Redemption::open(root.path(), &credential.account_id, &ready.confirmation).unwrap();
            assert!(operation.fresh);
            expected.push(operation.request_id.clone());
            let outcome = send_redemption(&client, &url, &credential, &operation.request_id)
                .await
                .unwrap();
            assert_eq!(outcome, want);
            assert!(outcome.settled() && !outcome.redeemed());
            operation.finish(outcome).unwrap();
            drop(operation);
            assert!(
                Redemption::open(root.path(), &credential.account_id, &ready.confirmation).is_err()
            );
            assert_eq!(
                current(root.path(), &credential.account_id).status,
                OperationStatus::Ready
            );
        }
        assert_eq!(server.join().unwrap(), expected);
    }

    /// 5xx、409、认证失败、连接断开：结果未知，保留同一标识，下次重试仍用它。
    #[tokio::test]
    async fn undetermined_http_failures_keep_the_pending_id() {
        let (url, server) = serve(vec![
            Some((503, json!({ "error": "unavailable" }))),
            Some((500, Value::Null)),
            Some((409, json!({ "error": "in progress" }))),
            Some((401, json!({ "error": "unauthorized" }))),
            None,
            Some((200, json!({ "code": "reset" }))),
        ]);
        let root = tempfile::tempdir().unwrap();
        let client = test_client();
        let credential = test_credential();
        let first = open_current(root.path(), &credential.account_id);
        let request_id = first.request_id.clone();
        drop(first);
        let pending = current(root.path(), &credential.account_id);
        for _ in 0..5 {
            let operation =
                Redemption::open(root.path(), &credential.account_id, &pending.confirmation)
                    .unwrap();
            assert_eq!(operation.request_id, request_id);
            assert!(
                send_redemption(&client, &url, &credential, &operation.request_id)
                    .await
                    .is_err()
            );
        }
        let mut operation =
            Redemption::open(root.path(), &credential.account_id, &pending.confirmation).unwrap();
        assert_eq!(operation.request_id, request_id);
        let outcome = send_redemption(&client, &url, &credential, &operation.request_id)
            .await
            .unwrap();
        assert_eq!(outcome, ConsumeOutcome::Reset);
        operation.finish(outcome).unwrap();
        drop(operation);
        assert!(
            Redemption::open(root.path(), &credential.account_id, &pending.confirmation).is_err()
        );
        assert_eq!(server.join().unwrap(), vec![request_id; 6]);
    }

    #[test]
    fn abandoning_ends_the_pending_operation_and_never_reuses_its_id() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let ready = current(root, "account-a").confirmation;
        assert!(abandon_in(root, "account-a", &ready).is_err());
        let first = open_current(root, "account-a");
        let first_id = first.request_id.clone();
        drop(first);
        let prior = current(root, "account-a").confirmation;
        let busy = Redemption::open(root, "account-a", &prior).unwrap();
        assert!(abandon_in(root, "account-a", &prior).is_err());
        drop(busy);
        abandon_in(root, "account-a", &prior).unwrap();
        assert!(Redemption::open(root, "account-a", &prior).is_err());
        let fresh = open_current(root, "account-a");
        let fresh_id = fresh.request_id.clone();
        assert_ne!(fresh_id, first_id);
        drop(fresh);
        // 另一窗口仍确认旧操作，不能放弃或使用当前记录。
        assert!(abandon_in(root, "account-a", &prior).is_err());
        assert!(Redemption::open(root, "account-a", &prior).is_err());
        let active = current(root, "account-a").confirmation;
        assert!(abandon_in(root, "account-b", &active).is_err());
        assert_eq!(open_current(root, "account-a").request_id, fresh_id);
    }

    #[test]
    fn abandoning_repairs_a_corrupt_journal() {
        let root = tempfile::tempdir().unwrap();
        let first = open_current(root.path(), "account-a");
        let path = first.path.clone();
        drop(first);
        fs::write(&path, b"broken").unwrap();
        let damaged = current(root.path(), "account-a").confirmation;
        fs::write(&path, b"changed damage").unwrap();
        assert!(abandon_in(root.path(), "account-a", &damaged).is_err());
        let updated = current(root.path(), "account-a").confirmation;
        assert!(Redemption::open(root.path(), "account-a", &updated).is_err());
        abandon_in(root.path(), "account-a", &updated).unwrap();
        let prior_ready = current(root.path(), "account-a").confirmation;
        let repair_test = open_current(root.path(), "account-a");
        assert!(repair_test.fresh);
        drop(repair_test);
        fs::write(&path, b"changed damage").unwrap();
        let next_damage = current(root.path(), "account-a").confirmation;
        abandon_in(root.path(), "account-a", &next_damage).unwrap();
        assert!(Redemption::open(root.path(), "account-a", &prior_ready).is_err());
        assert!(open_current(root.path(), "account-a").fresh);
    }

    #[test]
    fn journal_without_settled_list_still_loads() {
        let root = tempfile::tempdir().unwrap();
        let first = open_current(root.path(), "account-a");
        let path = first.path.clone();
        drop(first);
        fs::write(&path, br#"{"pending":"one","completed":[]}"#).unwrap();
        let retry = open_current(root.path(), "account-a");
        assert_eq!(retry.request_id, "one");
        assert!(!retry.fresh);
    }
}
