use std::time::Duration;

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use anyhow::{Context, bail};
use base64::Engine;
use futures_util::StreamExt;
use md5::{Digest, Md5};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

const CHANNEL_VERSION: &str = "codex-plus-weixin/1.0";
const MAX_REPLY_CHARS: usize = 3_800;
const MAX_API_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SMALL_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 25 * 1024 * 1024;
const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

#[derive(Debug, Clone)]
pub struct WeixinClient {
    base_url: String,
    token: String,
    route_tag: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeixinQrCode {
    #[serde(rename = "qrcode")]
    pub qr_code: String,
    #[serde(rename = "qrcode_img_content")]
    pub qr_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WeixinQrStatus {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub ilink_bot_id: String,
    #[serde(default)]
    pub baseurl: String,
    #[serde(default)]
    pub ilink_user_id: String,
    #[serde(default)]
    pub redirect_host: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeixinUpdates {
    #[serde(default)]
    pub ret: i64,
    #[serde(default)]
    pub errcode: i64,
    #[serde(default)]
    pub errmsg: String,
    #[serde(default, rename = "msgs")]
    pub messages: Vec<WeixinMessage>,
    #[serde(default)]
    pub get_updates_buf: String,
    #[serde(default)]
    pub longpolling_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeixinMessage {
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub message_id: i64,
    #[serde(default)]
    pub from_user_id: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub create_time_ms: i64,
    #[serde(default)]
    pub message_type: i64,
    #[serde(default)]
    pub message_state: i64,
    #[serde(default)]
    pub item_list: Vec<WeixinMessageItem>,
    #[serde(default)]
    pub context_token: String,
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub chat_room_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeixinMessageItem {
    #[serde(default, rename = "type")]
    pub item_type: i64,
    #[serde(default)]
    pub text_item: Option<WeixinTextItem>,
    #[serde(default)]
    pub voice_item: Option<WeixinVoiceItem>,
    #[serde(default)]
    pub ref_msg: Option<WeixinReferenceMessage>,
    #[serde(default)]
    pub image_item: Option<serde_json::Value>,
    #[serde(default)]
    pub file_item: Option<serde_json::Value>,
    #[serde(default)]
    pub video_item: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeixinTextItem {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeixinVoiceItem {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeixinReferenceMessage {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message_item: Option<Box<WeixinMessageItem>>,
}

#[derive(Debug, Deserialize)]
struct WeixinSendResponse {
    #[serde(default)]
    ret: i64,
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    errmsg: String,
}

impl WeixinClient {
    pub fn new(base_url: &str, token: &str, route_tag: &str) -> anyhow::Result<Self> {
        let base_url = normalize_base_url(base_url);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("无法创建微信 HTTP 客户端")?;
        Ok(Self {
            base_url,
            token: token.trim().to_string(),
            route_tag: route_tag.trim().to_string(),
            client,
        })
    }

    pub async fn fetch_qr_code(base_url: &str, route_tag: &str) -> anyhow::Result<WeixinQrCode> {
        let client = Self::new(base_url, "", route_tag)?;
        let mut url = client.endpoint("ilink/bot/get_bot_qrcode")?;
        url.query_pairs_mut().append_pair("bot_type", "3");
        let response = client
            .client
            .get(url)
            .headers(client.route_headers(false)?)
            .timeout(Duration::from_secs(40))
            .send()
            .await
            .context("获取微信登录二维码失败")?;
        let (status, bytes) =
            read_response_limited(response, MAX_SMALL_RESPONSE_BYTES, "微信二维码").await?;
        if !status.is_success() {
            bail!("获取微信登录二维码失败：HTTP {status}");
        }
        let qr: WeixinQrCode = serde_json::from_slice(&bytes).context("微信二维码响应格式无效")?;
        if qr.qr_code.trim().is_empty() || qr.qr_content.trim().is_empty() {
            bail!("微信二维码响应缺少必要字段");
        }
        Ok(qr)
    }

    pub async fn poll_qr_status(
        base_url: &str,
        route_tag: &str,
        qr_code: &str,
    ) -> anyhow::Result<WeixinQrStatus> {
        let client = Self::new(base_url, "", route_tag)?;
        let mut url = client.endpoint("ilink/bot/get_qrcode_status")?;
        url.query_pairs_mut().append_pair("qrcode", qr_code);
        let response = client
            .client
            .get(url)
            .headers(client.route_headers(true)?)
            .timeout(Duration::from_secs(40))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Ok(WeixinQrStatus {
                    status: "wait".to_string(),
                    ..WeixinQrStatus::default()
                });
            }
            Err(error) => return Err(error).context("查询微信扫码状态失败"),
        };
        let (status, bytes) =
            read_response_limited(response, MAX_SMALL_RESPONSE_BYTES, "微信扫码状态").await?;
        if !status.is_success() {
            bail!("查询微信扫码状态失败：HTTP {status}");
        }
        serde_json::from_slice(&bytes).context("微信扫码状态响应格式无效")
    }

    pub async fn get_updates(
        &self,
        get_updates_buf: &str,
        timeout_ms: u64,
    ) -> anyhow::Result<WeixinUpdates> {
        let request_body = json!({
            "get_updates_buf": get_updates_buf,
            "base_info": { "channel_version": CHANNEL_VERSION }
        });
        let timeout_ms = timeout_ms.clamp(1_000, 60_000);
        let response = self
            .client
            .post(self.endpoint("ilink/bot/getupdates")?)
            .headers(self.auth_headers()?)
            .json(&request_body)
            .timeout(Duration::from_millis(timeout_ms + 5_000))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Ok(WeixinUpdates {
                    ret: 0,
                    errcode: 0,
                    errmsg: String::new(),
                    messages: Vec::new(),
                    get_updates_buf: get_updates_buf.to_string(),
                    longpolling_timeout_ms: timeout_ms,
                });
            }
            Err(error) => return Err(error).context("微信长轮询请求失败"),
        };
        let (http_status, bytes) =
            read_response_limited(response, MAX_API_RESPONSE_BYTES, "微信长轮询").await?;
        if !http_status.is_success() {
            bail!("微信长轮询请求失败：HTTP {http_status}");
        }
        let updates: WeixinUpdates =
            serde_json::from_slice(&bytes).context("微信长轮询响应格式无效")?;
        if updates.ret != 0 || updates.errcode != 0 {
            bail!(
                "微信长轮询被拒绝：ret={} errcode={} {}",
                updates.ret,
                updates.errcode,
                updates.errmsg
            );
        }
        Ok(updates)
    }

    async fn post_api(
        &self,
        endpoint: &str,
        mut body: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        body["base_info"] = json!({ "channel_version": CHANNEL_VERSION });
        let response = self
            .client
            .post(self.endpoint(endpoint)?)
            .headers(self.auth_headers()?)
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .with_context(|| format!("微信 {endpoint} 请求失败"))?;
        let (status, bytes) =
            read_response_limited(response, MAX_SMALL_RESPONSE_BYTES, "微信 API").await?;
        if !status.is_success() {
            bail!("微信 {endpoint} 返回 HTTP {status}");
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("微信 API 响应格式无效")?;
        let ret = value["ret"].as_i64().unwrap_or(0);
        let errcode = value["errcode"].as_i64().unwrap_or(0);
        if ret != 0 || errcode != 0 {
            bail!("微信 {endpoint} 被拒绝：ret={ret} errcode={errcode}");
        }
        Ok(value)
    }

    pub async fn send_typing(
        &self,
        peer: &str,
        context_token: &str,
        active: bool,
    ) -> anyhow::Result<()> {
        let config = self
            .post_api(
                "ilink/bot/getconfig",
                json!({
                    "ilink_user_id": peer, "context_token": context_token
                }),
            )
            .await?;
        let ticket = config["typing_ticket"].as_str().unwrap_or_default();
        if ticket.is_empty() {
            return Ok(());
        }
        self.post_api(
            "ilink/bot/sendtyping",
            json!({
                "ilink_user_id": peer, "typing_ticket": ticket,
                "status": if active { 1 } else { 2 }
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn download_media(
        &self,
        item: &serde_json::Value,
        image_aeskey: Option<&str>,
    ) -> anyhow::Result<Vec<u8>> {
        let media = &item["media"];
        let encrypted_query_param = media["encrypt_query_param"].as_str().unwrap_or_default();
        let url = if !encrypted_query_param.is_empty() {
            let mut url = reqwest::Url::parse(&format!("{CDN_BASE_URL}/download"))?;
            url.query_pairs_mut()
                .append_pair("encrypted_query_param", encrypted_query_param);
            url
        } else {
            let full_url = media["full_url"].as_str().context("微信媒体缺少下载地址")?;
            checked_cdn_url(full_url)?
        };
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .context("下载微信媒体失败")?;
        let (status, mut bytes) =
            read_response_limited(response, MAX_MEDIA_BYTES, "微信媒体").await?;
        if !status.is_success() {
            bail!("下载微信媒体失败：HTTP {status}");
        }
        let key = if let Some(hex) = image_aeskey.filter(|value| !value.is_empty()) {
            parse_hex_key(hex)?
        } else if let Some(encoded) = media["aes_key"].as_str().filter(|value| !value.is_empty()) {
            parse_media_key(encoded)?
        } else {
            return Ok(bytes);
        };
        decrypt_media(&mut bytes, &key)?;
        Ok(bytes)
    }

    pub async fn send_file(
        &self,
        peer: &str,
        context_token: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let bytes = std::fs::read(path).context("读取待发送文件失败")?;
        if bytes.len() > MAX_MEDIA_BYTES {
            bail!("附件超过 25 MiB 限制");
        }
        let key = Uuid::new_v4().into_bytes();
        let file_key = Uuid::new_v4().simple().to_string();
        let digest = format!("{:x}", Md5::digest(&bytes));
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("附件文件名无效")?;
        let is_image = matches!(
            path.extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif"
        );
        let mut encrypted = bytes.clone();
        encrypt_media(&mut encrypted, &key);
        let upload = self
            .post_api(
                "ilink/bot/getuploadurl",
                json!({
                    "filekey": file_key, "media_type": if is_image { 1 } else { 3 },
                    "to_user_id": peer, "rawsize": bytes.len(), "rawfilemd5": digest,
                    "filesize": encrypted.len(), "no_need_thumb": true,
                    "aeskey": hex_key(&key)
                }),
            )
            .await?;
        let upload_url =
            if let Some(full_url) = upload["upload_full_url"].as_str().filter(|s| !s.is_empty()) {
                checked_cdn_url(full_url)?
            } else {
                let param = upload["upload_param"]
                    .as_str()
                    .context("微信未返回上传地址")?;
                let mut url = reqwest::Url::parse(&format!("{CDN_BASE_URL}/upload"))?;
                url.query_pairs_mut()
                    .append_pair("encrypted_query_param", param)
                    .append_pair("filekey", &file_key);
                url
            };
        let response = self
            .client
            .post(upload_url)
            .header("Content-Type", "application/octet-stream")
            .body(encrypted.clone())
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .context("上传微信附件失败")?;
        if !response.status().is_success() {
            bail!("上传微信附件失败：HTTP {}", response.status());
        }
        let encrypted_param = response
            .headers()
            .get("x-encrypted-param")
            .and_then(|header| header.to_str().ok())
            .filter(|value| !value.is_empty())
            .context("微信附件上传缺少 encrypted-param")?
            .to_string();
        let aes_key = base64::engine::general_purpose::STANDARD.encode(hex_key(&key));
        let media = json!({ "encrypt_query_param": encrypted_param, "aes_key": aes_key, "encrypt_type": 1 });
        let item = if is_image {
            json!({ "type": 2, "image_item": { "media": media, "mid_size": encrypted.len() } })
        } else {
            json!({ "type": 4, "file_item": { "media": media, "file_name": filename, "len": bytes.len().to_string() } })
        };
        self.post_api("ilink/bot/sendmessage", json!({
            "msg": { "from_user_id": "", "to_user_id": peer, "client_id": Uuid::new_v4().to_string(),
                "message_type": 2, "message_state": 2, "item_list": [item], "context_token": context_token },
            "base_info": { "channel_version": CHANNEL_VERSION }
        })).await?;
        Ok(())
    }

    pub async fn send_text_chunks(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: &str,
    ) -> anyhow::Result<()> {
        if context_token.trim().is_empty() {
            bail!("微信回复缺少 context_token");
        }
        for chunk in chunk_text(text, MAX_REPLY_CHARS) {
            self.send_text(to_user_id, &chunk, context_token).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    async fn send_text(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: &str,
    ) -> anyhow::Result<()> {
        let request_body = json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to_user_id,
                "client_id": Uuid::new_v4().to_string(),
                "message_type": 2,
                "message_state": 2,
                "item_list": [{
                    "type": 1,
                    "text_item": { "text": text }
                }],
                "context_token": context_token
            },
            "base_info": { "channel_version": CHANNEL_VERSION }
        });
        let response = self
            .client
            .post(self.endpoint("ilink/bot/sendmessage")?)
            .headers(self.auth_headers()?)
            .json(&request_body)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .context("发送微信回复失败")?;
        let (http_status, bytes) =
            read_response_limited(response, MAX_SMALL_RESPONSE_BYTES, "微信发送响应").await?;
        if !http_status.is_success() {
            bail!("发送微信回复失败：HTTP {http_status}");
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let result: WeixinSendResponse =
            serde_json::from_slice(&bytes).context("微信发送响应格式无效")?;
        if result.ret != 0 || result.errcode != 0 {
            bail!(
                "发送微信回复被拒绝：ret={} errcode={} {}",
                result.ret,
                result.errcode,
                result.errmsg
            );
        }
        Ok(())
    }

    fn endpoint(&self, path: &str) -> anyhow::Result<reqwest::Url> {
        let base = format!("{}/", self.base_url.trim_end_matches('/'));
        reqwest::Url::parse(&base)?
            .join(path.trim_start_matches('/'))
            .context("微信 API 地址无效")
    }

    fn auth_headers(&self) -> anyhow::Result<HeaderMap> {
        let mut headers = self.route_headers(false)?;
        headers.insert(
            "authorizationtype",
            HeaderValue::from_static("ilink_bot_token"),
        );
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token))?,
        );
        headers.insert("x-wechat-uin", HeaderValue::from_str(&random_wechat_uin())?);
        Ok(headers)
    }

    fn route_headers(&self, qr_status: bool) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("ilink-app-id", HeaderValue::from_static("bot"));
        headers.insert(
            "ilink-app-clientversion",
            HeaderValue::from_static("131584"),
        );
        if qr_status {
            headers.insert("ilink-app-clientversion", HeaderValue::from_static("1"));
        }
        if !self.route_tag.is_empty() {
            headers.insert("skroutetag", HeaderValue::from_str(&self.route_tag)?);
        }
        Ok(headers)
    }
}

pub fn render_qr_svg(content: &str) -> anyhow::Result<String> {
    let code = qrcode::QrCode::new(content.trim().as_bytes()).context("无法编码微信登录二维码")?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(320, 320)
        .dark_color(qrcode::render::svg::Color("#111827"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

fn checked_cdn_url(value: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).context("微信媒体地址无效")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some(
                "novac2c.cdn.weixin.qq.com"
                    | "ilinkai.weixin.qq.com"
                    | "wx.qlogo.cn"
                    | "thirdwx.qlogo.cn"
                    | "res.wx.qq.com"
                    | "mmbiz.qpic.cn"
                    | "mmbiz.qlogo.cn"
            )
        )
    {
        bail!("微信媒体地址不在允许的 CDN 域名内");
    }
    Ok(url)
}

fn parse_hex_key(value: &str) -> anyhow::Result<[u8; 16]> {
    if value.len() != 32 {
        bail!("微信媒体 AES 密钥长度无效");
    }
    let mut key = [0_u8; 16];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .context("微信媒体 AES 密钥格式无效")?;
    }
    Ok(key)
}

fn parse_media_key(value: &str) -> anyhow::Result<[u8; 16]> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(value)
        .context("微信媒体 AES 密钥编码无效")?;
    if raw.len() == 16 {
        return raw
            .try_into()
            .map_err(|_| anyhow::anyhow!("微信媒体 AES 密钥长度无效"));
    }
    let hex = std::str::from_utf8(&raw).context("微信媒体 AES 密钥格式无效")?;
    parse_hex_key(hex)
}

fn hex_key(key: &[u8; 16]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encrypt_media(bytes: &mut Vec<u8>, key: &[u8; 16]) {
    let pad = 16 - bytes.len() % 16;
    bytes.extend(std::iter::repeat_n(pad as u8, pad));
    let cipher = Aes128::new(GenericArray::from_slice(key));
    for block in bytes.chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
}

fn decrypt_media(bytes: &mut Vec<u8>, key: &[u8; 16]) -> anyhow::Result<()> {
    if bytes.is_empty() || bytes.len() % 16 != 0 {
        bail!("微信媒体密文长度无效");
    }
    let cipher = Aes128::new(GenericArray::from_slice(key));
    for block in bytes.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    let pad = *bytes.last().unwrap() as usize;
    if pad == 0
        || pad > 16
        || !bytes[bytes.len() - pad..]
            .iter()
            .all(|byte| *byte as usize == pad)
    {
        bail!("微信媒体解密校验失败");
    }
    bytes.truncate(bytes.len() - pad);
    Ok(())
}

async fn read_response_limited(
    response: reqwest::Response,
    max_bytes: usize,
    label: &str,
) -> anyhow::Result<(reqwest::StatusCode, Vec<u8>)> {
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("读取{label}失败"))?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            bail!("{label}超过大小限制");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}

impl WeixinMessage {
    pub fn is_finished_user_message(&self) -> bool {
        self.message_type == 1 && self.message_state == 2 && !self.from_user_id.trim().is_empty()
    }

    pub fn text(&self) -> Option<String> {
        self.item_list.iter().find_map(message_item_text)
    }

    pub fn dedup_key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.from_user_id.trim(),
            self.message_id,
            self.seq,
            self.create_time_ms,
            self.client_id.trim()
        )
    }

    pub fn is_older_than(&self, now_ms: u64, max_age_ms: u64) -> bool {
        self.create_time_ms > 0 && now_ms.saturating_sub(self.create_time_ms as u64) > max_age_ms
    }
}

fn message_item_text(item: &WeixinMessageItem) -> Option<String> {
    let body = match item.item_type {
        1 => item.text_item.as_ref().map(|item| item.text.trim()),
        3 => item.voice_item.as_ref().map(|item| item.text.trim()),
        _ => None,
    }?;
    if body.is_empty() {
        return None;
    }
    let Some(reference) = item.ref_msg.as_ref() else {
        return Some(body.to_string());
    };
    let mut reference_parts = Vec::new();
    if !reference.title.trim().is_empty() {
        reference_parts.push(reference.title.trim().to_string());
    }
    if let Some(reference_item) = reference.message_item.as_deref()
        && let Some(reference_text) = message_item_text(reference_item)
    {
        reference_parts.push(reference_text);
    }
    if reference_parts.is_empty() {
        Some(body.to_string())
    } else {
        Some(format!("[引用: {}]\n{body}", reference_parts.join(" | ")))
    }
}

fn normalize_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        super::DEFAULT_WEIXIN_BASE_URL.to_string()
    } else {
        value.to_string()
    }
}

fn random_wechat_uin() -> String {
    let bytes = Uuid::new_v4().into_bytes();
    let number = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    base64::engine::general_purpose::STANDARD.encode(number.to_string())
}

fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 || text.is_empty() {
        return Vec::new();
    }
    let chars = text.chars().collect::<Vec<_>>();
    chars
        .chunks(max_chars)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_and_voice_transcription() {
        let text: WeixinMessage = serde_json::from_value(json!({
            "message_type": 1,
            "message_state": 2,
            "from_user_id": "peer",
            "item_list": [{"type": 1, "text_item": {"text": " hello "}}]
        }))
        .unwrap();
        assert_eq!(text.text().as_deref(), Some("hello"));

        let voice: WeixinMessage = serde_json::from_value(json!({
            "message_type": 1,
            "message_state": 2,
            "from_user_id": "peer",
            "item_list": [{"type": 3, "voice_item": {"text": "语音文字"}}]
        }))
        .unwrap();
        assert_eq!(voice.text().as_deref(), Some("语音文字"));
    }

    #[test]
    fn chunks_unicode_by_character_count() {
        assert_eq!(chunk_text("一二三四五", 2), vec!["一二", "三四", "五"]);
    }

    #[test]
    fn dedup_key_distinguishes_zero_message_ids() {
        let first: WeixinMessage = serde_json::from_value(json!({
            "from_user_id": "peer",
            "message_id": 0,
            "seq": 0,
            "create_time_ms": 100,
            "client_id": "client-a"
        }))
        .unwrap();
        let second: WeixinMessage = serde_json::from_value(json!({
            "from_user_id": "peer",
            "message_id": 0,
            "seq": 0,
            "create_time_ms": 101,
            "client_id": "client-b"
        }))
        .unwrap();
        assert_ne!(first.dedup_key(), second.dedup_key());
    }

    #[test]
    fn renders_qr_as_svg_without_remote_service() {
        let svg = render_qr_svg("https://example.test/login").unwrap();
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn media_crypto_round_trip_and_rejects_untrusted_url() {
        let key = [7_u8; 16];
        let mut bytes = b"private image data".to_vec();
        encrypt_media(&mut bytes, &key);
        assert_ne!(bytes, b"private image data");
        decrypt_media(&mut bytes, &key).unwrap();
        assert_eq!(bytes, b"private image data");
        assert!(checked_cdn_url("https://evil.example/media").is_err());
        assert!(checked_cdn_url("http://novac2c.cdn.weixin.qq.com/media").is_err());
    }
}
