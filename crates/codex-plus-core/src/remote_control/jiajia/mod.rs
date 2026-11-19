//! 加加：远程控制里的聊天机器人，替用户照看 Codex 里的会话。
//!
//! 怎么跑的：
//! - 「大脑」是侧车 app-server 上的一个**临时会话**（`ephemeral`，不落盘，桌面侧栏看不到）。
//!   `baseInstructions` 换成加加自己的提示词，只读沙箱、从不审批；
//!   工具是一个只挂给这个会话的 stdio MCP（`--codex-plus-jiajia-mcp`），它把调用转回本服务的
//!   `/api/jiajia/tool`（专用令牌，只认本机），真正干活的是 [`tools`]，复用远程控制已有的会话操作。
//! - 聊天记录、记忆、回访、盯着的会话都在 [`store`]（状态目录下的文件）。会话是临时的，
//!   侧车重启、上下文太长、清空聊天时换一个新会话，把记忆和最近的聊天放进 `developerInstructions` 接上。
//! - 后台每几秒看一眼最近有动静的会话（[`events`]）：等审批直接在聊天里贴审批卡片（不花模型调用）；
//!   出错、跑完按设置攒一会儿再一次交给加加，或者只发固定格式的提醒。

pub mod events;
pub mod mcp;
pub mod prompt;
pub mod store;
pub mod tools;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use tokio::sync::watch;

use self::events::{Event, EventKind, Pacer, Phase, Snapshot};
use self::store::{ChatMessage, DoneReports, Store, now_ms};
use super::devices::{Principal, Role};
use super::service::RemoteService;
use super::sidecar::Sidecar;

/// MCP 进程的启动参数（管理器主程序认这个参数，见 `main.rs`）。
pub const MCP_FLAG: &str = "--codex-plus-jiajia-mcp";
/// MCP 进程从这两个环境变量拿转发地址和令牌（只在加加会话的配置覆盖里，不写进 config.toml）。
pub const ENV_URL: &str = "CODEX_PLUS_JIAJIA_URL";
pub const ENV_TOKEN: &str = "CODEX_PLUS_JIAJIA_TOKEN";
pub const MCP_SERVER_ID: &str = "jiajia";
/// 微信里发给加加时记的发送方（网页上显示「微信」）：微信桥和 `focus_thread` 工具都靠它认出「本人在微信里说的话」。
pub const WEIXIN_SENDER: &str = "微信";
/// `focus_thread` 发出的请求过了这么久还没被微信桥取走就作废（微信连接没开着）。
pub const FOCUS_REQUEST_TTL_MS: i64 = 2 * 60 * 1000;

/// 后台循环的节拍；看会话的间隔。
const TICK: Duration = Duration::from_secs(2);
const WATCH_EVERY_MS: i64 = 5_000;
/// 多久之内有更新的会话算「最近有动静」。
const RECENT_WINDOW_SECS: i64 = 20 * 60;
/// 一次最多看几个会话。
const WATCH_BATCH: usize = 12;
/// 上下文用到这个比例就换新会话（记忆和最近聊天会接上）。
const ROTATE_RATIO: f64 = 0.7;
/// 推送给网页时带多少条最近的记录。
const STATE_MESSAGES: usize = 80;

/// 加加让微信进入专注的请求（`focus_thread` 工具发出，微信桥取走后切到这个会话）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusRequest {
    pub thread_id: String,
    pub title: String,
    pub at: i64,
}

/// 交给加加的一条输入。
#[derive(Debug, Clone)]
enum Input {
    User(String, Principal, bool),
    Events(String),
    Later(String),
}

impl Input {
    fn text(&self) -> &str {
        match self {
            Input::User(text, _, _) | Input::Events(text) | Input::Later(text) => text,
        }
    }

    fn principal(&self) -> Option<&Principal> {
        match self {
            Self::User(_, principal, _) => Some(principal),
            Self::Events(_) | Self::Later(_) => None,
        }
    }

    fn from_weixin(&self) -> bool {
        matches!(self, Self::User(_, _, true))
    }
}

/// 不同设备的输入不能合并进同一个授权上下文；插话也只取当前设备的消息。
fn take_inputs(
    queue: &mut VecDeque<Input>,
    principal: Option<&Principal>,
    from_weixin: bool,
    steering: bool,
) -> Vec<Input> {
    let same_sender =
        |input: &Input| input.principal() == principal && input.from_weixin() == from_weixin;
    let mut inputs = Vec::new();
    if steering {
        let mut rest = VecDeque::new();
        for input in queue.drain(..) {
            if matches!(input, Input::User(..)) && same_sender(&input) {
                inputs.push(input);
            } else {
                rest.push_back(input);
            }
        }
        *queue = rest;
    } else {
        while queue.front().is_some_and(same_sender) {
            inputs.push(queue.pop_front().unwrap());
        }
    }
    inputs
}

fn granted_role(principal: Option<&Principal>, current: Option<Role>) -> Role {
    match principal {
        Some(Principal::Master) => Role::Full,
        Some(Principal::Device { role, .. }) => {
            current.map(|live| live.min(*role)).unwrap_or(Role::View)
        }
        None => Role::View,
    }
}

/// 加加背后那个临时会话。
struct BotThread {
    thread_id: String,
    sidecar: Weak<Sidecar>,
    turn_id: Option<String>,
    running: bool,
    /// 下一轮前换新会话（上下文太长、清空了聊天、改了记忆）。
    rotate: bool,
}

/// 被看着的会话：上次看到的更新时间和状态。
struct Tracked {
    updated_at: i64,
    snapshot: Snapshot,
}

#[derive(Default)]
struct Watcher {
    tracked: HashMap<String, Tracked>,
    /// 第一轮只记基线（等审批的除外），免得一开就把旧事全报一遍。
    primed: bool,
    last_ms: i64,
    pacer: Pacer,
}

pub struct Jiajia {
    service: Weak<RemoteService>,
    store: Mutex<Store>,
    bot: tokio::sync::Mutex<Option<BotThread>>,
    queue: Mutex<VecDeque<Input>>,
    watcher: tokio::sync::Mutex<Watcher>,
    /// thinking 时正在做什么（给网页显示「正在查看会话…」）。
    status: Mutex<(bool, Option<String>)>,
    /// 正在执行的输入所属设备；排队中的消息不能改变本轮权限。
    sender: Mutex<Option<Principal>>,
    /// 加加用 read_thread 看过的每个会话的最后一轮：这一轮的跑完 / 出错不用再告诉它（它已经知道了）。
    seen_ends: Mutex<HashMap<String, String>>,
    /// 最近一条用户消息是从哪儿来的（`by`）：`focus_thread` 只在用户是从微信说话时有效。
    active_from_weixin: AtomicBool,
    /// 等微信桥取走的「进入专注」请求。
    focus_request: Mutex<Option<FocusRequest>>,
    changed: watch::Sender<u64>,
    tool_token: String,
    port: AtomicU16,
    started: AtomicBool,
}

impl Jiajia {
    pub fn new(dir: PathBuf, service: Weak<RemoteService>) -> Arc<Self> {
        Arc::new(Self {
            service,
            store: Mutex::new(Store::open(&dir)),
            bot: tokio::sync::Mutex::new(None),
            queue: Mutex::new(VecDeque::new()),
            watcher: tokio::sync::Mutex::new(Watcher::default()),
            status: Mutex::new((false, None)),
            sender: Mutex::new(None),
            seen_ends: Mutex::new(HashMap::new()),
            active_from_weixin: AtomicBool::new(false),
            focus_request: Mutex::new(None),
            changed: watch::channel(0).0,
            tool_token: super::generate_token(),
            port: AtomicU16::new(0),
            started: AtomicBool::new(false),
        })
    }

    pub fn default_dir() -> PathBuf {
        crate::paths::default_app_state_dir().join("jiajia")
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn service(&self) -> anyhow::Result<Arc<RemoteService>> {
        self.service.upgrade().context("远程控制服务已停止")
    }

    pub fn tool_token(&self) -> &str {
        &self.tool_token
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn touch(&self) {
        self.changed.send_modify(|version| *version += 1);
    }

    pub fn sender_role(&self) -> Role {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let current = match &sender {
            Some(Principal::Device { id, .. }) => super::devices::list()
                .iter()
                .find(|device| device.id == *id)
                .map(|device| device.role),
            _ => None,
        };
        granted_role(sender.as_ref(), current)
    }

    /// 当前执行的回合是否来自已绑定的微信本人。
    pub fn last_sender_is_weixin(&self) -> bool {
        self.active_from_weixin
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 微信桥取走「进入专注」请求（取一次就没了；过期的丢掉）。
    pub fn take_focus_request(&self) -> Option<FocusRequest> {
        let request = self
            .focus_request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        (now_ms() - request.at <= FOCUS_REQUEST_TTL_MS).then_some(request)
    }

    fn request_focus(&self, thread_id: &str, title: &str) {
        *self.focus_request.lock().unwrap_or_else(|e| e.into_inner()) = Some(FocusRequest {
            thread_id: thread_id.to_string(),
            title: title.to_string(),
            at: now_ms(),
        });
        self.touch();
    }

    /// 加加读过这个会话：记下它看到的最后一轮。
    fn note_seen(&self, thread_id: &str, view: &Value) {
        if let Some(end) = events::snapshot(view).last_end {
            self.seen_ends
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(thread_id.to_string(), end.turn_id);
        }
    }

    /// 这条跑完 / 出错的事件，加加是不是已经从 read_thread 里看到了。
    fn already_seen(&self, event: &Event) -> bool {
        let Some(turn_id) = &event.turn_id else {
            return false;
        };
        self.seen_ends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&event.thread_id)
            == Some(turn_id)
    }

    /// 起后台循环（看会话、投递事件、回访）。重复调用无效。
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let Some(jiajia) = weak.upgrade() else { return };
                if jiajia.service.upgrade().is_none() {
                    return;
                }
                jiajia.tick().await;
            }
        });
    }

    // ---------- 给网页的状态 ----------

    /// 网页要的完整状态：设置、忙不忙、未读、最近的聊天。
    pub fn state(&self) -> Value {
        let store = self.store();
        let messages = store.page_before(None, STATE_MESSAGES);
        json!({
            "summary": self.summary_locked(&store),
            "messages": messages,
            "hasMore": store.messages.front().is_some_and(|first| messages.first().is_some_and(|shown| shown.id > first.id)),
        })
    }

    /// 列表页和推送用的摘要。
    pub fn summary(&self) -> Value {
        let store = self.store();
        self.summary_locked(&store)
    }

    fn summary_locked(&self, store: &Store) -> Value {
        let (thinking, activity) = self
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let last = store
            .messages
            .iter()
            .rev()
            .find(|message| message.role != "event");
        json!({
            "settings": store.settings,
            "thinking": thinking,
            "activity": activity,
            "unread": store.unread(),
            "readUpto": store.read_upto,
            "rev": store.rev(),
            "last": last.map(|message| json!({ "role": message.role, "text": events::clip(&message.text, 60), "at": message.at, "card": message.request.is_some() })),
            "watched": store.watched,
            "later": store.later,
        })
    }

    /// 比 `rev` 新的聊天记录（推送增量）。
    pub fn changed_since(&self, rev: u64) -> Vec<ChatMessage> {
        self.store().changed_since(rev)
    }

    pub fn page_before(&self, before: Option<u64>, limit: usize) -> Vec<ChatMessage> {
        self.store().page_before(before, limit.clamp(1, 200))
    }

    /// 聊天记录当前的位置：(变更序号, 最后一条的 id)。微信转发从这里开始，不重放旧记录。
    pub fn cursor(&self) -> (u64, u64) {
        let store = self.store();
        (
            store.rev(),
            store.messages.back().map(|message| message.id).unwrap_or(0),
        )
    }

    /// 第一次选过模型（没选过时 `send` 会拒绝）。
    pub fn is_configured(&self) -> bool {
        self.store().settings.configured
    }

    /// 在别处（微信）处理了审批卡片：标成已处理，网页上跟着灰掉。
    pub fn resolve_card(&self, id: u64) {
        let changed = self.store().update(id, |message| message.resolved = true);
        if changed {
            self.touch();
        }
    }

    pub fn mark_read(&self, upto: u64) {
        self.store().mark_read(upto);
        self.touch();
    }

    pub fn memory(&self) -> Value {
        let store = self.store();
        json!({ "user": store.user_memory, "notes": store.notes })
    }

    pub async fn set_memory(&self, body: &Value) -> anyhow::Result<Value> {
        {
            let mut store = self.store();
            if let Some(user) = body.get("user").and_then(Value::as_str) {
                store.user_memory = clip_chars(user, store::MEMORY_LIMIT);
            }
            if let Some(notes) = body.get("notes").and_then(Value::as_str) {
                store.notes = clip_chars(notes, store::MEMORY_LIMIT);
            }
            store.save_memory();
        }
        self.mark_rotate().await;
        Ok(self.memory())
    }

    /// 改设置：模型、推理强度、主动消息、跑完要不要报、性格、头像（文字 / 底色 / 恢复默认）；
    /// 第一次选完模型时 `configured: true`。性格写在会话的说明里，改了就换新会话。
    pub async fn update_settings(&self, body: &Value) -> anyhow::Result<Value> {
        let persona_changed = {
            let mut store = self.store();
            let before = store.settings.persona.clone();
            let mut settings = Self::apply_settings(store.settings.clone(), body)?;
            // 换成文字头像或恢复默认：旧图片删掉。
            if settings.avatar.image.is_none() {
                if let Some(old) = store.settings.avatar.image.clone() {
                    let _ = std::fs::remove_file(store.dir().join(old));
                }
            }
            if settings.avatar.image != store.settings.avatar.image
                || settings.avatar.text != store.settings.avatar.text
                || settings.avatar.color != store.settings.avatar.color
            {
                settings.avatar.version = store.settings.avatar.version + 1;
            }
            let changed = settings.persona != before;
            store.settings = settings;
            store.save_settings();
            changed
        };
        if persona_changed {
            self.mark_rotate().await;
        }
        self.touch();
        Ok(self.summary())
    }

    /// 存一张头像图片（网页已缩成 256×256）。只认图片文件头。
    pub fn set_avatar_image(&self, bytes: &[u8]) -> anyhow::Result<Value> {
        if bytes.len() > store::AVATAR_LIMIT {
            bail!("头像图片太大（上限 1 MB）");
        }
        let extension =
            store::avatar_extension(bytes).context("只支持 JPEG / PNG / GIF / WebP 图片")?;
        {
            let mut store = self.store();
            let name = format!("avatar.{extension}");
            if let Some(old) = store
                .settings
                .avatar
                .image
                .clone()
                .filter(|old| old != &name)
            {
                let _ = std::fs::remove_file(store.dir().join(old));
            }
            std::fs::write(store.dir().join(&name), bytes).context("保存头像失败")?;
            store.settings.avatar.image = Some(name);
            store.settings.avatar.version += 1;
            store.save_settings();
        }
        self.touch();
        Ok(self.summary())
    }

    /// 头像图片：(内容, Content-Type)。没设图片时返回 None。
    pub fn avatar_image(&self) -> Option<(Vec<u8>, &'static str)> {
        let store = self.store();
        let name = store.settings.avatar.image.clone()?;
        let bytes = std::fs::read(store.dir().join(&name)).ok()?;
        let content_type = match store::avatar_extension(&bytes)? {
            "jpg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            _ => "image/webp",
        };
        Some((bytes, content_type))
    }

    /// 把网页传来的改动套到设置上（纯函数，校验也在这里）。
    pub fn apply_settings(
        mut settings: store::Settings,
        body: &Value,
    ) -> anyhow::Result<store::Settings> {
        if let Some(model) = body.get("model") {
            settings.model = model
                .as_str()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string);
        }
        if let Some(effort) = body.get("effort") {
            settings.effort = effort
                .as_str()
                .map(str::trim)
                .filter(|effort| !effort.is_empty())
                .map(str::to_string);
        }
        if let Some(proactive) = body.get("proactive").and_then(Value::as_bool) {
            settings.proactive = proactive;
        }
        if let Some(reports) = body.get("doneReports") {
            settings.done_reports = serde_json::from_value::<DoneReports>(reports.clone())
                .context("doneReports 无效")?;
        }
        if body.get("configured").and_then(Value::as_bool) == Some(true) {
            settings.configured = true;
        }
        if let Some(persona) = body.get("persona") {
            let persona = persona.as_str().context("persona 要是文字")?.trim();
            if persona.chars().count() > store::PERSONA_LIMIT {
                bail!("性格描述最多 {} 字", store::PERSONA_LIMIT);
            }
            settings.persona = persona.to_string();
        }
        if let Some(avatar) = body.get("avatar") {
            if avatar.get("reset").and_then(Value::as_bool) == Some(true) {
                settings.avatar = store::Avatar {
                    version: settings.avatar.version,
                    ..Default::default()
                };
            } else {
                // 设文字 / 底色就是不用图片了。
                if let Some(text) = avatar.get("text") {
                    let text = text.as_str().map(str::trim).filter(|text| !text.is_empty());
                    if text.is_some_and(|text| text.chars().count() > 4) {
                        bail!("头像文字最多 4 个字符（一两个字或一个 emoji）");
                    }
                    settings.avatar.text = text.map(str::to_string);
                    settings.avatar.image = None;
                }
                if let Some(color) = avatar.get("color") {
                    let color = color.as_str().filter(|color| !color.is_empty());
                    if color.is_some_and(|color| {
                        !(color.len() == 7
                            && color.starts_with('#')
                            && color[1..].chars().all(|c| c.is_ascii_hexdigit()))
                    }) {
                        bail!("头像底色要是 #rrggbb");
                    }
                    settings.avatar.color = color.map(str::to_string);
                    settings.avatar.image = None;
                }
            }
        }
        Ok(settings)
    }

    /// 清空聊天：记忆、回访、盯着的会话保留，换一个新会话（不带旧聊天）。
    pub async fn clear_chat(&self) -> anyhow::Result<Value> {
        self.store().clear_chat();
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.mark_rotate().await;
        self.touch();
        Ok(self.state())
    }

    async fn mark_rotate(&self) {
        if let Some(bot) = self.bot.lock().await.as_mut() {
            bot.rotate = true;
        }
    }

    // ---------- 聊天 ----------

    /// 用户发来一条消息。
    pub async fn send(
        self: &Arc<Self>,
        text: &str,
        by: Option<String>,
        principal: Principal,
    ) -> anyhow::Result<Value> {
        self.enqueue_user(text, by, principal, false).await
    }

    pub(crate) async fn send_from_weixin(self: &Arc<Self>, text: &str) -> anyhow::Result<Value> {
        self.enqueue_user(text, Some(WEIXIN_SENDER.into()), Principal::Master, true)
            .await
    }

    async fn enqueue_user(
        self: &Arc<Self>,
        text: &str,
        by: Option<String>,
        principal: Principal,
        from_weixin: bool,
    ) -> anyhow::Result<Value> {
        let text = text.trim();
        if text.is_empty() {
            bail!("缺少 text");
        }
        if !self.store().settings.configured {
            bail!("先选一个模型");
        }
        let id = self.store().push(ChatMessage {
            id: 0,
            at: 0,
            role: "user".into(),
            text: text.to_string(),
            threads: vec![],
            request: None,
            resolved: false,
            by,
            rev: 0,
        });
        self.store().mark_read(id);
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Input::User(text.to_string(), principal, from_weixin));
        self.touch();
        let jiajia = self.clone();
        tokio::spawn(async move { jiajia.pump().await });
        Ok(json!({ "id": id }))
    }

    pub async fn interrupt(&self) -> anyhow::Result<Value> {
        let bot = self.bot.lock().await;
        let Some(bot) = bot.as_ref().filter(|bot| bot.running) else {
            bail!("加加现在没在回复")
        };
        let sidecar = bot.sidecar.upgrade().context("加加的会话已经不在了")?;
        let turn_id = bot.turn_id.clone().context("这一轮还没开始")?;
        sidecar
            .call(
                "turn/interrupt",
                json!({ "threadId": bot.thread_id, "turnId": turn_id }),
            )
            .await
    }

    fn push_message(
        &self,
        role: &str,
        text: String,
        threads: Vec<String>,
        request: Option<Value>,
    ) -> u64 {
        let id = self.store().push(ChatMessage {
            id: 0,
            at: 0,
            role: role.into(),
            text,
            threads,
            request,
            resolved: false,
            by: None,
            rev: 0,
        });
        self.touch();
        id
    }

    fn set_status(&self, thinking: bool, activity: Option<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = (thinking, activity);
        self.touch();
    }

    /// 有输入就交给加加：没在回复就开新一轮（攒着的输入合成一条）；正在回复时用户的话插进去。
    async fn pump(self: &Arc<Self>) {
        let mut bot = self.bot.lock().await;
        if bot.as_ref().is_some_and(|bot| bot.running) {
            let user: Vec<Input> = {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                let sender = self
                    .sender
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                take_inputs(
                    &mut queue,
                    sender.as_ref(),
                    self.last_sender_is_weixin(),
                    true,
                )
            };
            if user.is_empty() {
                return;
            }
            let text = user.iter().map(Input::text).collect::<Vec<_>>().join("\n");
            if let Some(running) = bot.as_ref() {
                let steered = match (running.sidecar.upgrade(), running.turn_id.clone()) {
                    (Some(sidecar), Some(turn_id)) => sidecar
                        .call(
                            "turn/steer",
                            json!({ "threadId": running.thread_id, "expectedTurnId": turn_id, "input": [{ "type": "text", "text": text, "text_elements": [] }] }),
                        )
                        .await
                        .is_ok(),
                    _ => false,
                };
                if !steered {
                    // 插不进去（这一轮刚好结束）：放回队首，等这一轮结束再发。
                    let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                    for input in user.into_iter().rev() {
                        queue.push_front(input);
                    }
                }
            }
            return;
        }
        let (inputs, sender, from_weixin) = {
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            let sender = queue.front().and_then(Input::principal).cloned();
            let from_weixin = queue.front().is_some_and(Input::from_weixin);
            (
                take_inputs(&mut queue, sender.as_ref(), from_weixin, false),
                sender,
                from_weixin,
            )
        };
        if inputs.is_empty() {
            return;
        }
        {
            let mut previous = self.sender.lock().unwrap_or_else(|e| e.into_inner());
            if *previous != sender || self.last_sender_is_weixin() != from_weixin {
                if let Some(bot) = bot.as_mut() {
                    bot.rotate = true;
                }
            }
            *previous = sender;
            self.active_from_weixin
                .store(from_weixin, std::sync::atomic::Ordering::SeqCst);
        }
        let result = async {
            self.ensure_thread(&mut bot).await?;
            let running = bot.as_mut().context("加加的会话没建起来")?;
            let sidecar = running.sidecar.upgrade().context("后台进程已退出")?;
            let now = now_ms();
            let mut text = format!("[{}]\n", prompt::format_now(now));
            text.push_str(&inputs.iter().map(Input::text).collect::<Vec<_>>().join("\n\n"));
            let settings = self.store().settings.clone();
            let mut params = json!({ "threadId": running.thread_id, "input": [{ "type": "text", "text": text, "text_elements": [] }] });
            if let Some(model) = &settings.model {
                params["model"] = json!(model);
            }
            if let Some(effort) = &settings.effort {
                params["effort"] = json!(effort);
            }
            let started = sidecar.call("turn/start", params).await?;
            running.turn_id = started.pointer("/turn/id").and_then(Value::as_str).map(str::to_string);
            running.running = true;
            anyhow::Ok(())
        }
        .await;
        drop(bot);
        match result {
            Ok(()) => self.set_status(true, None),
            Err(error) => {
                self.set_status(false, None);
                self.push_message(
                    "notice",
                    format!("加加没能开始回复：{error:#}"),
                    vec![],
                    None,
                );
            }
        }
    }

    /// 确保有一个能用的临时会话：没有、侧车换了、要换新的时新建。
    async fn ensure_thread(self: &Arc<Self>, bot: &mut Option<BotThread>) -> anyhow::Result<()> {
        let service = self.service()?;
        let sidecar = service.sidecar().await?;
        if let Some(existing) = bot.as_ref() {
            let same = existing
                .sidecar
                .upgrade()
                .is_some_and(|old| Arc::ptr_eq(&old, &sidecar) && old.is_alive());
            if same && !existing.rotate {
                return Ok(());
            }
        }
        let port = self.port.load(Ordering::SeqCst);
        if port == 0 {
            bail!("远程控制的本机入口没开");
        }
        // 换新会话：旧的那个放掉（临时会话不落盘，卸载就没了）。
        if let Some(old) = bot.take() {
            if let Some(old_sidecar) = old.sidecar.upgrade().filter(|old| old.is_alive()) {
                let thread_id = old.thread_id;
                tokio::spawn(async move {
                    let _ = old_sidecar
                        .call("thread/unsubscribe", json!({ "threadId": thread_id }))
                        .await;
                });
            }
        }
        let params = self.thread_start_params(&service, port)?;
        let started = sidecar
            .call("thread/start", params)
            .await
            .context("建加加的会话失败")?;
        let thread_id = started
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .context("thread/start 没有返回会话 ID")?
            .to_string();
        tokio::spawn(listen_boxed(
            Arc::downgrade(self),
            sidecar.events(),
            thread_id.clone(),
        ));
        *bot = Some(BotThread {
            thread_id,
            sidecar: Arc::downgrade(&sidecar),
            turn_id: None,
            running: false,
            rotate: false,
        });
        Ok(())
    }

    fn thread_start_params(&self, service: &RemoteService, port: u16) -> anyhow::Result<Value> {
        let exe = std::env::current_exe().context("找不到 Codex++ 主程序")?;
        let (settings, developer, workspace) = {
            let store = self.store();
            let now = now_ms();
            let recent = store.page_before(None, 20);
            let developer = prompt::developer_instructions(
                &prompt::format_now(now),
                &super::host_display_name(),
                &store.settings.persona,
                &store.user_memory,
                &store.notes,
                &store.watched,
                &store.later,
                &recent,
            );
            (
                store.settings.clone(),
                developer,
                store.dir().join("workspace"),
            )
        };
        std::fs::create_dir_all(&workspace).ok();
        let mut config = serde_json::Map::new();
        // MCP 工具直接给模型（不折叠到 tool_search 后面），调用不弹审批。
        config.insert("features.tool_search".into(), json!(false));
        let server = |key: &str| format!("mcp_servers.{MCP_SERVER_ID}.{key}");
        config.insert(server("command"), json!(exe.to_string_lossy()));
        config.insert(server("args"), json!([MCP_FLAG]));
        config.insert(
            server("env"),
            json!({ ENV_URL: format!("http://127.0.0.1:{port}/api/jiajia/tool"), ENV_TOKEN: self.tool_token }),
        );
        config.insert(server("default_tools_approval_mode"), json!("approve"));
        config.insert(server("startup_timeout_sec"), json!(20));
        config.insert(server("tool_timeout_sec"), json!(150));
        // 用户自己配的 MCP 不给加加（省上下文，也免得它去碰别的工具）。
        for id in configured_mcp_servers(&service.codex_home().join("config.toml")) {
            if id != MCP_SERVER_ID {
                config.insert(format!("mcp_servers.{id}.enabled"), json!(false));
            }
        }
        let mut params = json!({
            "cwd": workspace.to_string_lossy(),
            "ephemeral": true,
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "baseInstructions": prompt::BASE_INSTRUCTIONS,
            "developerInstructions": developer,
            "config": Value::Object(config),
        });
        if let Some(model) = &settings.model {
            params["model"] = json!(model);
        }
        Ok(params)
    }

    /// 收到加加会话的一条推送。
    async fn on_bot_event(self: &Arc<Self>, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "item/started" => {
                if let Some(activity) = params.get("item").and_then(activity_of) {
                    self.set_status(true, Some(activity));
                }
            }
            "item/completed" => {
                let Some(item) = params.get("item") else {
                    return;
                };
                if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                    return;
                }
                // 过程中的旁白（commentary）不发给用户。
                if item.get("phase").and_then(Value::as_str) == Some("commentary") {
                    return;
                }
                let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
                if let Some((text, threads)) = prompt::parse_reply(text) {
                    let threads = self.resolve_cards(&threads).await;
                    self.push_message("bot", text, threads, None);
                }
            }
            "thread/tokenUsage/updated" => {
                let usage = super::follower::token_usage(params.get("tokenUsage"));
                if let (Some(used), Some(window)) = (
                    usage.get("used").and_then(Value::as_u64),
                    usage.get("window").and_then(Value::as_u64),
                ) {
                    if window > 0 && used as f64 / window as f64 > ROTATE_RATIO {
                        self.mark_rotate().await;
                    }
                }
            }
            "turn/completed" => {
                {
                    let mut bot = self.bot.lock().await;
                    if let Some(bot) = bot.as_mut() {
                        bot.running = false;
                        bot.turn_id = None;
                    }
                }
                if let Some(error) = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
                {
                    self.push_message(
                        "notice",
                        format!("加加这一轮出错了：{}", events::clip(error, 300)),
                        vec![],
                        None,
                    );
                }
                self.set_status(false, None);
                let jiajia = self.clone();
                tokio::spawn(async move { jiajia.pump().await });
            }
            _ => {}
        }
    }

    /// 回复里的卡片标记可能只写了前 8 位：换成完整 ID，认不出的丢掉。
    async fn resolve_cards(&self, raw: &[String]) -> Vec<String> {
        let mut ids = Vec::new();
        for item in raw {
            if let Ok((id, _)) = self.resolve_thread(item).await {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    // ---------- 后台：看会话、投递事件、回访 ----------

    async fn tick(self: &Arc<Self>) {
        if !self.store().settings.configured {
            return;
        }
        let now = now_ms();
        let due_watch = {
            let mut watcher = self.watcher.lock().await;
            let due = now - watcher.last_ms >= WATCH_EVERY_MS;
            if due {
                watcher.last_ms = now;
            }
            due
        };
        if due_watch {
            let _ = self.watch_threads().await;
        }
        self.fire_later(now);
        self.deliver_events(now).await;
        if !self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            self.pump().await;
        }
    }

    /// 回访到点：交给加加。
    fn fire_later(&self, now: i64) {
        let due: Vec<store::Later> = {
            let mut store = self.store();
            let (due, rest): (Vec<_>, Vec<_>) =
                store.later.drain(..).partition(|item| item.due_ms <= now);
            store.later = rest;
            if !due.is_empty() {
                store.save_later();
            }
            due
        };
        if due.is_empty() {
            return;
        }
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        for item in due {
            queue.push_back(Input::Later(format!(
                "<回访 编号=\"{}\">{}</回访>",
                item.id, item.note
            )));
        }
        drop(queue);
        self.touch();
    }

    /// 攒够时间的事件一起交给加加（它正在回复时先不投）。
    async fn deliver_events(&self, now: i64) {
        if self
            .bot
            .lock()
            .await
            .as_ref()
            .is_some_and(|bot| bot.running)
        {
            return;
        }
        let batch = {
            let mut watcher = self.watcher.lock().await;
            if !watcher.pacer.is_due(now) {
                return;
            }
            watcher.pacer.take(now)
        };
        // 攒着的这段时间里加加可能已经自己读过了（比如刚派的活马上就看了结果）。
        let batch: Vec<Event> = batch
            .into_iter()
            .filter(|event| !self.already_seen(event))
            .collect();
        if batch.is_empty() {
            return;
        }
        let line = batch
            .iter()
            .map(|event| {
                format!(
                    "「{}」{}",
                    if event.title.is_empty() {
                        "未命名"
                    } else {
                        &event.title
                    },
                    event.kind.label()
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let threads: Vec<String> =
            batch
                .iter()
                .map(|event| event.thread_id.clone())
                .fold(Vec::new(), |mut ids, id| {
                    if !ids.contains(&id) && ids.len() < 3 {
                        ids.push(id);
                    }
                    ids
                });
        self.push_message("event", line, threads, None);
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Input::Events(events::format_batch(&batch)));
    }

    /// 看一眼最近有动静的会话，比对前后状态得出事件。
    async fn watch_threads(self: &Arc<Self>) -> anyhow::Result<()> {
        let service = self.service()?;
        let list = service.list_threads(None).await?;
        let threads = list
            .get("threads")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let now_secs = now_ms() / 1000;
        let watched: Vec<String> = self
            .store()
            .watched
            .iter()
            .map(|watched| watched.thread_id.clone())
            .collect();
        // 还挂着未处理审批卡片的会话也要看：服务重启后没有「上次的状态」可比，只能拿当前请求核对卡片。
        let carded = self.carded_threads();
        // 先在锁里挑出要看的会话和上次的状态；拉视图（跟随桌面最慢要等几秒）时不拿锁，工具调用不被卡住。
        let (primed, candidates, to_view) = {
            let watcher = self.watcher.lock().await;
            let mut candidates: Vec<(String, i64)> = Vec::new();
            for thread in &threads {
                let Some(id) = thread.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let updated = thread.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
                let busy = watcher
                    .tracked
                    .get(id)
                    .is_some_and(|tracked| tracked.snapshot.phase != Phase::Idle);
                if busy
                    || watched.iter().any(|watched| watched == id)
                    || carded.iter().any(|carded| carded == id)
                    || now_secs - updated <= RECENT_WINDOW_SECS
                {
                    candidates.push((id.to_string(), updated));
                }
            }
            candidates.truncate(WATCH_BATCH);
            let to_view: Vec<(String, i64, Option<Snapshot>)> = candidates
                .iter()
                .filter_map(|(id, updated)| {
                    let previous = watcher.tracked.get(id);
                    let unchanged = previous.is_some_and(|tracked| {
                        tracked.updated_at == *updated && tracked.snapshot.phase == Phase::Idle
                    });
                    (!unchanged).then(|| {
                        (
                            id.clone(),
                            *updated,
                            previous.map(|tracked| tracked.snapshot.clone()),
                        )
                    })
                })
                .collect();
            (watcher.primed, candidates, to_view)
        };
        let mut viewed: Vec<(String, i64, Option<Snapshot>, Snapshot)> = Vec::new();
        for (id, updated, previous) in to_view {
            let Ok(view) = service.thread_view(&id).await else {
                continue;
            };
            viewed.push((id, updated, previous, events::snapshot(&view)));
        }
        let mut watcher = self.watcher.lock().await;
        let mut found: Vec<Event> = Vec::new();
        let mut settled: Vec<(String, Vec<String>)> = Vec::new();
        for (id, updated, previous, next) in viewed {
            let is_watched = watched.iter().any(|watched| watched == &id);
            let mut changes =
                events::transitions(&id, previous.as_ref(), &next, is_watched && primed);
            if !primed {
                // 第一轮只报等审批（还没贴过卡片的），其余当基线。
                changes.retain(|event| event.kind == EventKind::Waiting);
            }
            settled.push((
                id.clone(),
                next.requests.iter().map(|(key, _)| key.clone()).collect(),
            ));
            if next.requests.is_empty() {
                watcher.pacer.drop_waiting(&id);
            }
            found.extend(changes);
            watcher.tracked.insert(
                id,
                Tracked {
                    updated_at: updated,
                    snapshot: next,
                },
            );
        }
        // 不再活跃的会话不用一直记着。
        let keep: Vec<&String> = candidates.iter().map(|(id, _)| id).collect();
        watcher
            .tracked
            .retain(|id, tracked| keep.contains(&id) || tracked.snapshot.phase != Phase::Idle);
        watcher.primed = true;
        let now = now_ms();
        let settings = self.store().settings.clone();
        for event in found {
            let routed = self.route(&event, &settings, &watched, &mut watcher.pacer, now);
            self.store().record_event(store::EventRecord {
                at: now,
                kind: event.kind.id().into(),
                thread_id: event.thread_id.clone(),
                title: event.title.clone(),
                cwd: event.cwd.clone(),
                detail: event.detail.clone(),
                routed: routed.into(),
            });
        }
        drop(watcher);
        for (thread_id, live) in settled {
            self.settle_request_cards(&thread_id, &live);
        }
        Ok(())
    }

    /// 还有未处理审批卡片的会话。
    fn carded_threads(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for message in self
            .store()
            .messages
            .iter()
            .filter(|message| !message.resolved)
        {
            if let Some(id) = message
                .request
                .as_ref()
                .and_then(|card| card.get("threadId"))
                .and_then(Value::as_str)
            {
                if !ids.iter().any(|known| known == id) {
                    ids.push(id.to_string());
                }
            }
        }
        ids
    }

    /// 一条事件怎么处理：贴卡片 / 告诉加加 / 只发提醒 / 只记录。
    fn route(
        &self,
        event: &Event,
        settings: &store::Settings,
        watched: &[String],
        pacer: &mut Pacer,
        now: i64,
    ) -> &'static str {
        match event.kind {
            EventKind::Waiting => {
                let request = event.request.clone().unwrap_or(Value::Null);
                let key = request.get("id").map(Value::to_string).unwrap_or_default();
                let exists = self.store().messages.iter().any(|message| {
                    message.request.as_ref().is_some_and(|card| {
                        card.get("threadId").and_then(Value::as_str)
                            == Some(event.thread_id.as_str())
                            && card.pointer("/request/id").map(Value::to_string)
                                == Some(key.clone())
                    })
                });
                if exists {
                    return "ignore";
                }
                self.push_message(
                    "notice",
                    events::notice_text(event),
                    vec![event.thread_id.clone()],
                    Some(json!({ "threadId": event.thread_id, "title": event.title, "request": request })),
                );
                "card"
            }
            EventKind::Failed | EventKind::Done => {
                if self.already_seen(event) {
                    return "seen";
                }
                let is_watched = watched.iter().any(|id| id == &event.thread_id);
                let wanted = event.kind == EventKind::Failed
                    || is_watched
                    || settings.done_reports == DoneReports::All;
                if !wanted
                    || (event.kind == EventKind::Done
                        && settings.done_reports == DoneReports::None
                        && !is_watched)
                {
                    return "ignore";
                }
                if settings.proactive {
                    pacer.add(event.clone(), now);
                    "tell"
                } else {
                    self.push_message(
                        "notice",
                        events::notice_text(event),
                        vec![event.thread_id.clone()],
                        None,
                    );
                    "notify"
                }
            }
        }
    }

    /// 拿会话当前还挂着的请求核对审批卡片：不在里面的都已处理（在哪儿处理的都算，会话被中断、服务重启也算）。
    fn settle_request_cards(&self, thread_id: &str, live: &[String]) {
        let ids: Vec<u64> = self
            .store()
            .messages
            .iter()
            .filter(|message| !message.resolved)
            .filter(|message| {
                message.request.as_ref().is_some_and(|card| {
                    card.get("threadId").and_then(Value::as_str) == Some(thread_id)
                        && card
                            .pointer("/request/id")
                            .is_some_and(|id| !live.contains(&id.to_string()))
                })
            })
            .map(|message| message.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let mut store = self.store();
        for id in ids {
            store.update(id, |message| message.resolved = true);
        }
        drop(store);
        self.touch();
    }

    /// 某个会话当前的状态（工具列会话时用；没看过的返回 None）。
    pub(crate) async fn known_phase(&self, thread_id: &str) -> Option<(Phase, Option<String>)> {
        let watcher = self.watcher.lock().await;
        watcher.tracked.get(thread_id).map(|tracked| {
            (
                tracked.snapshot.phase,
                tracked
                    .snapshot
                    .last_end
                    .as_ref()
                    .map(|end| end.status.clone()),
            )
        })
    }
}

/// 签名写死的装箱版本：`ensure_thread` → `listen` → `pump` → `ensure_thread` 互相引用，
/// 不装箱的话编译器推不出这几个异步函数的类型。
fn listen_boxed(
    jiajia: Weak<Jiajia>,
    events: tokio::sync::broadcast::Receiver<Value>,
    thread_id: String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(listen(jiajia, events, thread_id))
}

/// 只认加加会话的推送；会话换了（新会话 ID）或侧车退出就结束。
async fn listen(
    jiajia: Weak<Jiajia>,
    mut events: tokio::sync::broadcast::Receiver<Value>,
    thread_id: String,
) {
    loop {
        let message = match events.recv().await {
            Ok(message) => message,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if message.pointer("/params/threadId").and_then(Value::as_str) != Some(thread_id.as_str()) {
            continue;
        }
        let Some(jiajia) = jiajia.upgrade() else {
            return;
        };
        let current = jiajia
            .bot
            .lock()
            .await
            .as_ref()
            .map(|bot| bot.thread_id.clone());
        if current.as_deref() != Some(thread_id.as_str()) {
            return;
        }
        jiajia.on_bot_event(&message).await;
    }
    // 侧车退出：这一轮不会再有结束通知了。
    if let Some(jiajia) = jiajia.upgrade() {
        let mut bot = jiajia.bot.lock().await;
        if bot
            .as_ref()
            .is_some_and(|bot| bot.thread_id == thread_id && bot.running)
        {
            *bot = None;
            drop(bot);
            jiajia.set_status(false, None);
            jiajia.push_message(
                "notice",
                "后台进程退出了，加加这一轮没说完。再发一次试试。".into(),
                vec![],
                None,
            );
        }
    }
}

/// 正在调用的工具 → 网页上显示的「正在…」。
fn activity_of(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("mcpToolCall") {
        return None;
    }
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
    Some(tools::activity_label(tool).to_string())
}

/// config.toml 里配了哪些 MCP（给加加的会话关掉）。
pub fn configured_mcp_servers(config_path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    value
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
}

fn clip_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod authority_tests {
    use super::*;

    fn device(id: &str, role: Role) -> Principal {
        Principal::Device {
            id: id.into(),
            name: id.into(),
            role,
        }
    }

    #[test]
    fn queued_full_device_does_not_raise_message_turn_authority() {
        let message = device("phone", Role::Message);
        let full = device("desktop", Role::Full);
        let mut queue = VecDeque::from([
            Input::User("first".into(), message.clone(), false),
            Input::User("admin".into(), full.clone(), false),
            Input::User("later".into(), message.clone(), false),
        ]);
        let first = take_inputs(&mut queue, Some(&message), false, false);
        assert_eq!(first.len(), 1);
        let steering = take_inputs(&mut queue, Some(&message), false, true);
        assert_eq!(steering.len(), 1);
        assert_eq!(queue.front().unwrap().principal(), Some(&full));
        assert_eq!(
            granted_role(Some(&message), Some(Role::Message)),
            Role::Message
        );
    }

    #[test]
    fn weixin_input_cannot_steer_a_web_turn_from_the_same_owner() {
        let master = Principal::Master;
        let mut queue = VecDeque::from([
            Input::User("wechat".into(), master.clone(), true),
            Input::User("web".into(), master.clone(), false),
        ]);
        let web = take_inputs(&mut queue, Some(&master), false, true);
        assert_eq!(web[0].text(), "web");
        assert!(queue.front().unwrap().from_weixin());
    }

    #[tokio::test]
    async fn weixin_origin_is_active_only_after_its_input_is_processed() {
        let dir = tempfile::tempdir().unwrap();
        let jiajia = Jiajia::new(dir.path().to_path_buf(), Weak::new());
        jiajia
            .update_settings(&json!({ "configured": true }))
            .await
            .unwrap();
        jiajia.send_from_weixin("进入指定会话").await.unwrap();
        assert!(!jiajia.last_sender_is_weixin());
        jiajia.pump().await;
        assert!(jiajia.last_sender_is_weixin());
    }

    #[test]
    fn queued_authority_is_capped_by_current_device_permission() {
        let full = device("phone", Role::Full);
        let message = device("phone", Role::Message);
        assert_eq!(granted_role(Some(&full), None), Role::View);
        assert_eq!(
            granted_role(Some(&full), Some(Role::Message)),
            Role::Message
        );
        assert_eq!(
            granted_role(Some(&message), Some(Role::Full)),
            Role::Message
        );
        assert_eq!(granted_role(None, Some(Role::Full)), Role::View);
        assert_eq!(granted_role(Some(&Principal::Master), None), Role::Full);
    }
}
