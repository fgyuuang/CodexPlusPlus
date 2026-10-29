use base64::Engine;
use serde_json::{Map, Value, json};
use std::path::Path;

use crate::settings::BackendSettings;

const RENDERER_SCRIPT: &str = include_str!("../../../assets/inject/renderer-inject.js");
/// Agent 桌宠的按压 / 松开音效，按音效组内嵌：(组名, 按压音, 松开音)。
/// 音效随启动器本地内嵌，运行时不下载；桌宠图片则不内嵌，见 `agent_pet`。
const AGENT_PET_SOUNDS: &[(&str, &[u8], &[u8])] = &[
    (
        "duck",
        include_bytes!("../../../assets/inject/agent-pet/sounds/Ya1.mp3"),
        include_bytes!("../../../assets/inject/agent-pet/sounds/Ya2.mp3"),
    ),
    (
        "fx1",
        include_bytes!("../../../assets/inject/agent-pet/sounds/D1.mp3"),
        include_bytes!("../../../assets/inject/agent-pet/sounds/D2.mp3"),
    ),
];

/// 全部音效组：`{ 组名: { press, release } }`，值为 data URI。管理器试听与 Codex 注入共用。
pub fn agent_pet_sounds() -> Value {
    let values: serde_json::Map<String, Value> = AGENT_PET_SOUNDS
        .iter()
        .map(|(name, press, release)| {
            (
                (*name).to_string(),
                json!({
                    "press": image_data_uri("audio/mpeg", press),
                    "release": image_data_uri("audio/mpeg", release),
                }),
            )
        })
        .collect();
    Value::Object(values)
}

fn agent_pet_sounds_json() -> String {
    agent_pet_sounds().to_string()
}

fn agent_pet_sounds_script() -> String {
    format!(
        "window.__CODEX_PLUS_AGENT_PET_SOUNDS__ = {};",
        agent_pet_sounds_json()
    )
}

#[cfg(windows)]
const DREAM_TARGET_CSS: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/windows/dream-skin.css");
#[cfg(not(windows))]
const DREAM_TARGET_CSS: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/macos/dream-skin.css");
#[cfg(windows)]
const DREAM_TARGET_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/windows/renderer-inject.js");
#[cfg(not(windows))]
const DREAM_TARGET_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/macos/renderer-inject.js");
#[cfg(windows)]
const CIDALA_TARGET_CSS: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/windows/dream-skin.css");
#[cfg(not(windows))]
const CIDALA_TARGET_CSS: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/macos/dream-skin.css");
#[cfg(windows)]
const CIDALA_TARGET_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/windows/renderer-inject.js");
#[cfg(not(windows))]
const CIDALA_TARGET_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/macos/renderer-inject.js");
const CODEX_SNOW_CSS: &str =
    include_str!("../../../assets/inject/upstream/snow-skin/dream-skin.css");
const CODEX_SNOW_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/snow-skin/renderer-inject.js");
const GLASS_VISION_CSS: &str =
    include_str!("../../../assets/inject/upstream/glass-vision/glass-vision.css");
const GLASS_VISION_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/glass-vision/renderer-inject.js");
const DREAM_SKIN_COMMUNITY_COMPAT_CSS: &str =
    include_str!("../../../assets/inject/dream-skin-community-compat.css");
#[cfg(windows)]
const DREAM_SKIN_DEFAULT_IMAGE: &[u8] =
    include_bytes!("../../../assets/inject/upstream/dream-skin/windows/dream-reference.jpg");
#[cfg(not(windows))]
const DREAM_SKIN_DEFAULT_IMAGE: &[u8] =
    include_bytes!("../../../assets/inject/upstream/dream-skin/macos/portal-hero.png");
const PET_REAL_MOUSE_SCRIPT: &str = include_str!("../../../assets/inject/pet-real-mouse-inject.js");
const API_QUOTA_GATE_SCRIPT: &str = include_str!("../../../assets/inject/api-quota-gate.js");
const NATIVE_FAST_UNLOCK_SCRIPT: &str =
    include_str!("../../../assets/inject/native-fast-unlock.js");
const STEPWISE_SCRIPT: &str = concat!(
    "(() => {\n",
    include_str!("../../../assets/inject/floating-panel/runtime/state.js"),
    include_str!("../../../assets/inject/floating-panel/core/appearance-runtime.js"),
    include_str!("../../../assets/inject/floating-panel/runtime/dom.js"),
    include_str!("../../../assets/inject/floating-panel/runtime/bridge-client.js"),
    include_str!("../../../assets/inject/floating-panel/runtime/answer-context.js"),
    include_str!("../../../assets/inject/floating-panel/stepwise/suggestions.js"),
    include_str!("../../../assets/inject/floating-panel/stepwise/generation.js"),
    include_str!("../../../assets/inject/floating-panel/runtime/lifecycle.js"),
    include_str!("../../../assets/inject/floating-panel/runtime/settings.js"),
    include_str!("../../../assets/inject/floating-panel/core/appearance.js"),
    include_str!("../../../assets/inject/floating-panel/core/host.js"),
    include_str!("../../../assets/inject/floating-panel/core/geometry.js"),
    include_str!("../../../assets/inject/floating-panel/core/interaction.js"),
    include_str!("../../../assets/inject/floating-panel/core/views.js"),
    include_str!("../../../assets/inject/floating-panel/outline/parser.js"),
    include_str!("../../../assets/inject/floating-panel/outline/navigation.js"),
    include_str!("../../../assets/inject/floating-panel/outline/feature.js"),
    include_str!("../../../assets/inject/floating-panel/outline/view.js"),
    include_str!("../../../assets/inject/floating-panel/core/scroll-state.js"),
    include_str!("../../../assets/inject/floating-panel-inject.js"),
    "\n})();\n",
);
pub const DIAGNOSTIC_BUILD_ID: &str = "diag-20260518-1";
const DREAM_SKIN_RENDERER_REVISION: &str = "27-dream-skin-1.5.20-wallpaper-shell";

pub fn renderer_script() -> &'static str {
    RENDERER_SCRIPT
}

pub fn dream_skin_default_image() -> (&'static str, &'static [u8]) {
    #[cfg(windows)]
    return ("image/jpeg", DREAM_SKIN_DEFAULT_IMAGE);
    #[cfg(not(windows))]
    return ("image/png", DREAM_SKIN_DEFAULT_IMAGE);
}

pub fn dream_skin_art_data_uri(settings: &BackendSettings) -> String {
    if !settings.codex_app_dream_skin_enabled {
        return String::new();
    }
    let custom_path = settings.codex_app_dream_skin_image_path.trim();
    if !custom_path.is_empty()
        && crate::dream_skin::is_managed_dream_skin_image(
            Path::new(custom_path),
            &crate::paths::default_app_state_dir(),
        )
        && let Some(data_uri) = image_file_data_uri(Path::new(custom_path))
    {
        return data_uri;
    }
    let (content_type, image) = dream_skin_default_image();
    image_data_uri(content_type, image)
}

fn dream_skin_platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "macos" }
}

fn uses_cidala_target_engine(style_preset: &str) -> bool {
    matches!(
        style_preset,
        "midnight-aurora" | "amber-dusk" | "forest-mist" | "cyber-neon" | "sakura-dawn"
    )
}

fn dream_skin_target_assets(
    settings: &BackendSettings,
) -> (&'static str, &'static str, &'static str) {
    let theme = &settings.codex_app_dream_skin_theme_config;
    let style_preset =
        crate::settings::resolve_dream_skin_style_preset(&theme.id, &theme.style_preset);
    match style_preset.as_str() {
        "codex-snow" => ("snow", CODEX_SNOW_RENDERER, CODEX_SNOW_CSS),
        "glass-vision" => ("glass-vision", GLASS_VISION_RENDERER, GLASS_VISION_CSS),
        value if uses_cidala_target_engine(value) => {
            ("cidala-tiger", CIDALA_TARGET_RENDERER, CIDALA_TARGET_CSS)
        }
        _ => ("dream-skin", DREAM_TARGET_RENDERER, DREAM_TARGET_CSS),
    }
}

fn dream_skin_target_runtime_script(settings: &BackendSettings, include_art: bool) -> String {
    if !settings.codex_app_dream_skin_enabled || settings.codex_app_dream_skin_paused {
        return String::new();
    }

    let (engine, renderer, base_css) = dream_skin_target_assets(settings);
    let managed_css = managed_dream_skin_css(settings);
    // 原生 shell 兼容属于基础主题：内置图片和没有 Safe CSS 的图片也必须可达。
    let native_shell_css =
        include_str!("../../../assets/inject/dream-skin-native-shell-compat.css");
    let css = format!("{base_css}\n{native_shell_css}\n{managed_css}");
    let theme = serde_json::to_string(&settings.codex_app_dream_skin_theme_config)
        .expect("dream skin target theme should serialize");
    let style_revision = dream_skin_content_signature(css.as_bytes());
    let payload_revision =
        dream_skin_target_payload_signature(settings, engine, &style_revision, &theme);
    let mut payload = renderer
        .replace("__DREAM_CSS_JSON__", &serde_json::to_string(&css).unwrap())
        .replace("__DREAM_ART_JSON__", "window.__CODEX_PLUS_DREAM_SKIN_ART__")
        .replace(
            "__DREAM_THEME_JSON__",
            "window.__CODEX_PLUS_DREAM_SKIN_THEME__",
        )
        .replace(
            "__DREAM_VERSION_JSON__",
            &serde_json::to_string("2.1.0-snow.1").unwrap(),
        )
        .replace(
            "__GLASS_VISION_CSS_JSON__",
            &serde_json::to_string(&css).unwrap(),
        )
        .replace(
            "__GLASS_VISION_ART_JSON__",
            "window.__CODEX_PLUS_DREAM_SKIN_ART__",
        )
        .replace(
            "__DREAM_SKIN_CSS_JSON__",
            &serde_json::to_string(&css).unwrap(),
        )
        .replace(
            "__DREAM_SKIN_ART_JSON__",
            "window.__CODEX_PLUS_DREAM_SKIN_ART__",
        )
        .replace(
            "__DREAM_SKIN_THEME_JSON__",
            "window.__CODEX_PLUS_DREAM_SKIN_THEME__",
        )
        .replace(
            "__DREAM_SKIN_VERSION_JSON__",
            &serde_json::to_string(crate::dream_skin_package::DREAM_SKIN_PACKAGE_CLIENT_VERSION)
                .unwrap(),
        )
        .replace(
            "__DREAM_SKIN_STYLE_REVISION_JSON__",
            &serde_json::to_string(&style_revision).unwrap(),
        )
        .replace(
            "__DREAM_SKIN_PAYLOAD_REVISION_JSON__",
            &serde_json::to_string(&payload_revision).unwrap(),
        );
    if payload.contains("__DREAM_") || payload.contains("__GLASS_VISION_") {
        panic!("dream skin target renderer contains unresolved placeholders");
    }

    let art_assignment = include_art.then(|| {
        format!(
            "window.__CODEX_PLUS_DREAM_SKIN_ART__ = {};\n",
            serde_json::to_string(&dream_skin_art_data_uri(settings))
                .expect("dream skin target art should serialize")
        )
    });
    let skin_api_bootstrap = dream_skin_skin_api_bootstrap_script(&theme);
    payload = format!(
        "(() => {{\nwindow.__CODEX_PLUS_EXTERNAL_DREAM_SKIN_RUNTIME__ = true;\nwindow.__CODEX_PLUS_CLEAR_DREAM_SKIN__?.();\n{}window.__CODEX_PLUS_DREAM_SKIN_ART_SIGNATURE__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_THEME__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_RUNTIME_REVISION__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_TARGET_ENGINE__ = {};\n{}const result = {};\nconst state = window.__CODEX_DREAM_SKIN_STATE__ || window.__CODEX_GLASS_VISION_SKIN_STATE__;\nif (state) {{\n  state.version = `codex-plus:${{String(window.__CODEX_PLUS_DREAM_SKIN_PLATFORM__ || 'unknown')}}:${{window.__CODEX_PLUS_DREAM_SKIN_TARGET_ENGINE__}}:r${{window.__CODEX_PLUS_DREAM_SKIN_RUNTIME_REVISION__}}`;\n}}\nwindow.__CODEX_PLUS_DREAM_SKIN_PAYLOAD_SIGNATURE__ = {};\nreturn result;\n}})()",
        art_assignment.unwrap_or_default(),
        serde_json::to_string(&dream_skin_art_content_signature(settings)).unwrap(),
        theme,
        serde_json::to_string(DREAM_SKIN_RENDERER_REVISION).unwrap(),
        serde_json::to_string(engine).unwrap(),
        skin_api_bootstrap,
        payload,
        serde_json::to_string(&payload_revision).unwrap(),
    );
    payload
}

fn managed_dream_skin_css(settings: &BackendSettings) -> String {
    let image_path = settings.codex_app_dream_skin_image_path.trim();
    if image_path.is_empty()
        || !crate::dream_skin::is_managed_dream_skin_image(
            Path::new(image_path),
            &crate::paths::default_app_state_dir(),
        )
    {
        return String::new();
    }
    let css_path = Path::new(image_path)
        .parent()
        .map(|parent| parent.join("current.css"));
    let Some(css_path) = css_path else {
        return String::new();
    };
    let Ok(css) = std::fs::read_to_string(css_path) else {
        return String::new();
    };
    let Ok(compiled) = crate::dream_skin_package::compile_safe_css(&css) else {
        return String::new();
    };
    format!("{compiled}\n{DREAM_SKIN_COMMUNITY_COMPAT_CSS}")
}

fn dream_skin_skin_api_bootstrap_script(theme: &str) -> String {
    format!(
        r#"(() => {{
  const theme = {};
  const root = document.documentElement;
  const colors = theme && typeof theme.colors === "object" ? theme.colors : {{}};
  const variables = {{
    "--ds-theme-color-background": colors.background,
    "--ds-theme-color-panel": colors.panel,
    "--ds-theme-color-panel-alt": colors.panelAlt,
    "--ds-theme-color-accent": colors.accent,
    "--ds-theme-color-accent-alt": colors.accentAlt,
    "--ds-theme-color-secondary": colors.secondary,
    "--ds-theme-color-highlight": colors.highlight,
    "--ds-theme-color-text": colors.text,
    "--ds-theme-color-muted": colors.muted,
    "--ds-theme-color-line": colors.line,
    "--ds-theme-image-focus-x": String(theme?.art?.focusX ?? 0.5),
    "--ds-theme-image-focus-y": String(theme?.art?.focusY ?? 0.5),
  }};
  for (const [name, value] of Object.entries(variables)) if (typeof value === "string" && value) root.style.setProperty(name, value);
  const map = {{
    root: "html", sidebar: "aside.app-shell-left-panel", main: "main.main-surface",
    header: "header.app-header-tint", home: ".dream-skin-home, [data-feature='game-source']",
    "home-hero": ".dream-skin-home > div:first-child, [data-feature='game-source']",
    "project-list": "[data-feature='game-source']", thread: "main.main-surface [role='main']",
    message: "main.main-surface article", composer: ".composer-surface-chrome",
    "composer-toolbar": ".composer-surface-chrome [role='toolbar']", dialog: "[role='dialog']",
  }};
  const mark = () => {{
    for (const [part, selector] of Object.entries(map)) for (const node of document.querySelectorAll(selector)) {{
      // data-ds-part 是皮肤 API 的挂载点标记，值不变时绝不重写，避免长会话里对每条消息重复置属性
      if (node.getAttribute("data-ds-part") !== part) node.setAttribute("data-ds-part", part);
    }}
  }};
  mark();
  window.__CODEX_PLUS_DREAM_SKIN_API_OBSERVER__?.disconnect?.();
  let markTimer = null;
  const scheduleMark = () => {{
    if (markTimer !== null) return;
    markTimer = setTimeout(() => {{
      markTimer = null;
      mark();
    }}, 250);
  }};
  const observer = new MutationObserver((records) => {{
    // 流式输出只产生纯文本节点增删，不会增减皮肤挂载点；这类批次直接跳过（issue #2181）
    if (records.every((record) =>
      [...record.addedNodes].every((node) => node.nodeType === 3)
      && [...record.removedNodes].every((node) => node.nodeType === 3))) return;
    scheduleMark();
  }});
  observer.observe(document.documentElement, {{ childList: true, subtree: true }});
  window.__CODEX_PLUS_DREAM_SKIN_API_OBSERVER__ = observer;
}})();"#,
        theme,
    )
}

fn dream_skin_target_payload_signature(
    settings: &BackendSettings,
    engine: &str,
    style_revision: &str,
    theme: &str,
) -> String {
    dream_skin_content_signature(
        format!(
            "{engine}:{style_revision}:{}:{theme}",
            dream_skin_art_content_signature(settings)
        )
        .as_bytes(),
    )
}

pub fn stepwise_script() -> &'static str {
    STEPWISE_SCRIPT
}

pub fn pet_real_mouse_script() -> &'static str {
    PET_REAL_MOUSE_SCRIPT
}

pub fn api_quota_gate_script() -> &'static str {
    API_QUOTA_GATE_SCRIPT
}

pub fn native_fast_unlock_script() -> &'static str {
    NATIVE_FAST_UNLOCK_SCRIPT
}

const PET_V2_SPRITE_DETECTION_SCRIPT: &str = r#"
  const isV2Sprite = async (mascot) => {
    if (!mascot) return false;
    if (Array.from(mascot.querySelectorAll("img")).some((image) =>
      image.naturalWidth === 1536 && image.naturalHeight === 2288
    )) return true;
    for (const element of [mascot, ...mascot.querySelectorAll("*")]) {
      const background = getComputedStyle(element).backgroundImage || "";
      const match = background.match(/url\(["']?([^"')]+)/i);
      if (!match) continue;
      const source = match[1];
      const cacheKey = "__codexPlusPetV2SpriteProbe";
      let probe = window[cacheKey];
      if (!probe || probe.source !== source) {
        probe = { source, valid: false, pending: true };
        probe.promise = (async () => {
          try {
            const image = new Image();
            image.src = source;
            await image.decode();
            return image.naturalWidth === 1536 && image.naturalHeight === 2288;
          } catch {
            return false;
          }
        })().then((valid) => {
          probe.valid = valid;
          probe.pending = false;
          return valid;
        });
        window[cacheKey] = probe;
      }
      const wasPending = probe.pending;
      const valid = wasPending ? await probe.promise : probe.valid;
      if (wasPending) {
        const currentBackground = getComputedStyle(element).backgroundImage || "";
        const currentMatch = currentBackground.match(/url\(["']?([^"')]+)/i);
        if (currentMatch?.[1] !== source) continue;
      }
      if (window[cacheKey] === probe && valid) return true;
    }
    return false;
  };
"#;

pub fn pet_real_mouse_hook_probe_script() -> &'static str {
    r#"typeof window.__codexPlusPetRealMouseLook?.updateScreenPoint === "function""#
}

pub fn pet_real_mouse_capability_probe_script() -> String {
    let mut script = String::from(
        r#"
(async () => {
  const mascot = document.querySelector('[data-avatar-mascot="true"]');
"#,
    );
    script.push_str(PET_V2_SPRITE_DETECTION_SCRIPT);
    script.push_str(
        r#"
  if (!await isV2Sprite(mascot)) return false;
  const urls = [
    ...Array.from(document.scripts || []).map((script) => script.src),
    ...Array.from(document.querySelectorAll("link[href]") || []).map((link) => link.href),
    ...performance.getEntriesByType("resource").map((entry) => entry.name),
  ].filter((url) => url && url.includes("/assets/") && url.split("?")[0].endsWith(".js"));
  let dispatcherUrl = urls.find((url) => url.includes("vscode-api-"));
  if (!dispatcherUrl) {
    for (const url of urls) {
      try {
        const source = await fetch(url).then((response) => response.ok ? response.text() : "");
        const match = source.match(/["'](\.\/(?:assets\/)?vscode-api-[^"']+\.js)["']/);
        if (match) {
          dispatcherUrl = new URL(match[1], url).href;
          break;
        }
      } catch {
      }
    }
  }
  if (!dispatcherUrl) return false;
  try {
    const module = await import(dispatcherUrl);
    return Object.values(module || {}).some((value) => value
      && typeof value.dispatchHostMessage === "function"
      && typeof value.subscribe === "function");
  } catch {
    return false;
  }
})()
"#,
    );
    script
}

pub fn pet_real_mouse_update_script(x: i32, y: i32) -> String {
    // 只要页面上有 updateScreenPoint 就推坐标。不能绑 V2 精灵图或 vscode dispatcher。
    format!(
        "window.__codexPlusPetRealMouseLook?.updateScreenPoint?.({{ x: {x}, y: {y} }}) === true;"
    )
}

pub fn pet_real_mouse_stop_script() -> &'static str {
    "window.__codexPlusPetRealMouseLook?.stop?.();"
}

pub fn injection_script(helper_port: u16) -> String {
    injection_script_with_settings(helper_port, &BackendSettings::default())
}

pub fn hide_official_usage_alert_config(settings: &BackendSettings) -> bool {
    let profile = settings.active_relay_profile();
    profile.relay_mode == crate::settings::RelayMode::Official && profile.official_mix_api_key
}

pub fn force_chinese_locale_config(settings: &BackendSettings) -> Value {
    json!({ "enabled": settings.codex_app_force_chinese_locale, "locale": "zh-CN" })
}

fn renderer_after_document_ready(script: &str) -> String {
    format!(
        "(()=>{{const start=()=>{{{script}\n}};if(document.body)start();else document.addEventListener('DOMContentLoaded',start,{{once:true}});}})();"
    )
}

pub fn injection_script_with_settings(helper_port: u16, settings: &BackendSettings) -> String {
    let helper_url = format!("http://127.0.0.1:{helper_port}");
    let instance_label = std::env::var("CODEX_PLUS_INSTANCE_LABEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("CODEX_PLUS_INSTANCE_ID")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    let instance_identity = instance_label
        .as_deref()
        .and_then(|value| serde_json::to_string(value).ok())
        .map(|value| format!("window.__CODEX_PLUS_INSTANCE_LABEL__ = {value};\n"))
        .unwrap_or_default();
    let local_runtime_config = format!(
        "window.__CODEX_PLUS_PLUGIN_MARKETPLACES__ = {};\nwindow.__CODEX_PLUS_FAST_STARTUP__ = {};\n",
        serde_json::to_string(&local_plugin_marketplaces()).expect("marketplaces serialize"),
        serde_json::to_string(&fast_startup_config(settings)).expect("fast startup serialize"),
    );
    let image_overlay = image_overlay_config(helper_port, settings);
    let dream_skin_art = dream_skin_art_data_uri(settings);
    let dream_skin_art_signature = dream_skin_art_content_signature(settings);
    let dream_skin_theme = &settings.codex_app_dream_skin_theme_config;
    let dream_skin_target_runtime = dream_skin_target_runtime_script(settings, false);
    let paste_fix = paste_fix_enabled_config(settings);
    let force_chinese_locale = force_chinese_locale_config(settings);
    let hide_official_usage_alert = hide_official_usage_alert_config(settings);
    // Fast 的新文档脚本需要 Page.enable；DOM 增强必须等正常文档就绪，不能访问空的根节点。
    let renderer_runtime = if settings.enhancements_enabled && settings.codex_app_native_fast_unlock
    {
        renderer_after_document_ready(renderer_script())
    } else {
        renderer_script().to_string()
    };
    let stepwise_runtime =
        if settings.codex_app_stepwise_enabled || settings.codex_app_answer_outline_enabled {
            stepwise_script()
        } else {
            ""
        };
    format!(
        "{}window.__CODEX_SESSION_DELETE_HELPER__ = {};\nwindow.__CODEX_PLUS_VERSION__ = {};\nwindow.__CODEX_PLUS_BUILD__ = {};\nwindow.__CODEX_PLUS_IMAGE_OVERLAY__ = {};\nwindow.__CODEX_PLUS_EXTERNAL_DREAM_SKIN_RUNTIME__ = true;\nwindow.__CODEX_PLUS_DREAM_SKIN_PLATFORM__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_REVISION__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_ART__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_ART_SIGNATURE__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN_THEME__ = {};\nwindow.__CODEX_PLUS_PASTE_FIX__ = {};\nwindow.__CODEX_PLUS_FORCE_CHINESE_LOCALE__ = {};\nwindow.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = {};\n{}\n{}\n{}",
        format!("{instance_identity}{local_runtime_config}"),
        serde_json::to_string(&helper_url).expect("helper URL should serialize"),
        serde_json::to_string(crate::version::VERSION).expect("version should serialize"),
        serde_json::to_string(DIAGNOSTIC_BUILD_ID).expect("build id should serialize"),
        serde_json::to_string(&image_overlay).expect("image overlay config should serialize"),
        serde_json::to_string(dream_skin_platform()).expect("dream skin platform should serialize"),
        serde_json::to_string(DREAM_SKIN_RENDERER_REVISION)
            .expect("dream skin renderer revision should serialize"),
        serde_json::to_string(&dream_skin_art).expect("dream skin art should serialize"),
        serde_json::to_string(&dream_skin_art_signature)
            .expect("dream skin art signature should serialize"),
        serde_json::to_string(dream_skin_theme).expect("dream skin theme should serialize"),
        serde_json::to_string(&paste_fix).expect("paste fix config should serialize"),
        serde_json::to_string(&force_chinese_locale)
            .expect("force Chinese locale config should serialize"),
        serde_json::to_string(&hide_official_usage_alert)
            .expect("usage alert config should serialize"),
        format!(
            "{}\n{}\n{}\n{}",
            agent_pet_sounds_script(),
            include_str!("../../../assets/inject/composer-readiness.js"),
            format!(
                "{}\nwindow.__codexPlusNativeFastUnlock.runtime.update({});",
                native_fast_unlock_script(),
                json!(crate::native_fast::runtime_settings(settings)),
            ),
            renderer_runtime
        ),
        stepwise_runtime,
        dream_skin_target_runtime,
    )
}

pub fn dream_skin_live_update_probe_script() -> String {
    format!(
        "(() => {{ const state = window.__CODEX_DREAM_SKIN_STATE__ || window.__CODEX_GLASS_VISION_SKIN_STATE__; if (window.__CODEX_PLUS_DREAM_SKIN_RUNTIME_REVISION__ !== {} || !state) return null; state.ensure?.(); return JSON.stringify({{ artSignature: String(window.__CODEX_PLUS_DREAM_SKIN_ART_SIGNATURE__ || ''), payloadSignature: String(window.__CODEX_PLUS_DREAM_SKIN_PAYLOAD_SIGNATURE__ || '') }}); }})()",
        serde_json::to_string(DREAM_SKIN_RENDERER_REVISION)
            .expect("dream skin renderer revision should serialize")
    )
}

pub fn dream_skin_live_update_script(settings: &BackendSettings, include_art: bool) -> String {
    dream_skin_target_runtime_script(settings, include_art)
}

pub fn dream_skin_art_content_signature(settings: &BackendSettings) -> String {
    let custom_path = settings.codex_app_dream_skin_image_path.trim();
    if !custom_path.is_empty()
        && crate::dream_skin::is_managed_dream_skin_image(
            Path::new(custom_path),
            &crate::paths::default_app_state_dir(),
        )
        && let Ok(bytes) = std::fs::read(custom_path)
    {
        return dream_skin_content_signature(&bytes);
    }
    dream_skin_content_signature(DREAM_SKIN_DEFAULT_IMAGE)
}

pub fn dream_skin_runtime_content_signature(settings: &BackendSettings) -> String {
    let (engine, _, css) = dream_skin_target_assets(settings);
    let theme = serde_json::to_string(&settings.codex_app_dream_skin_theme_config)
        .expect("dream skin target theme should serialize");
    let style_revision = dream_skin_content_signature(css.as_bytes());
    dream_skin_target_payload_signature(settings, engine, &style_revision, &theme)
}

fn dream_skin_content_signature(value: &[u8]) -> String {
    let hash = value.iter().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
    });
    format!("{}-{hash:x}", value.len())
}

fn local_plugin_marketplaces() -> Value {
    let home = crate::codex_home::default_codex_home_dir();
    local_plugin_marketplaces_from_home(&home)
}

fn local_plugin_marketplaces_from_home(home: &Path) -> Value {
    let installed_plugins = installed_plugins_from_config(&home);
    let marketplace_dir = home
        .join(".tmp")
        .join("plugins")
        .join(".agents")
        .join("plugins");
    let mut candidates = vec![
        marketplace_dir.join("marketplace.json"),
        marketplace_dir.join("api_marketplace.json"),
        home.join(".tmp")
            .join("plugins-remote")
            .join(".agents")
            .join("plugins")
            .join("marketplace.json"),
        home.join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join(".agents")
            .join("plugins")
            .join("marketplace.json"),
    ];
    let sources_root = home.join("plugins").join("sources");
    let mut seen_third_party_roots = std::collections::BTreeSet::new();
    collect_local_marketplace_candidates(
        &mut candidates,
        &mut seen_third_party_roots,
        &home.join(".tmp").join("marketplaces"),
        false,
    );
    collect_local_marketplace_candidates(
        &mut candidates,
        &mut seen_third_party_roots,
        &sources_root,
        true,
    );
    let marketplaces = candidates
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            let mut marketplace: Value = serde_json::from_str(&text).ok()?;
            expand_local_plugin_marketplace(&mut marketplace, path, &home, &installed_plugins);
            if let Some(object) = marketplace.as_object_mut() {
                object
                    .entry("path")
                    .or_insert_with(|| Value::String(path.to_string_lossy().to_string()));
            }
            Some(marketplace)
        })
        .collect::<Vec<_>>();
    Value::Array(marketplaces)
}

fn collect_local_marketplace_candidates(
    candidates: &mut Vec<std::path::PathBuf>,
    seen_roots: &mut std::collections::BTreeSet<std::path::PathBuf>,
    parent: &Path,
    from_sources_root: bool,
) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let name = root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        if skip_local_marketplace_root(&name, from_sources_root) {
            continue;
        }
        let marketplace_path = root
            .join(".agents")
            .join("plugins")
            .join("marketplace.json");
        let api_marketplace_path = root
            .join(".agents")
            .join("plugins")
            .join("api_marketplace.json");
        if marketplace_path.is_file() && seen_roots.insert(root.clone()) {
            candidates.push(marketplace_path);
        }
        if api_marketplace_path.is_file() && seen_roots.insert(root.clone()) {
            candidates.push(api_marketplace_path);
        }
    }
}

fn skip_local_marketplace_root(name: &str, from_sources_root: bool) -> bool {
    if name.is_empty() || name.starts_with('.') || name == "echobird-ai" {
        return true;
    }
    from_sources_root
        && matches!(
            name,
            "openai-bundled" | "openai-primary-runtime" | "openai-bundled-fixed"
        )
}

fn expand_local_plugin_marketplace(
    marketplace: &mut Value,
    marketplace_path: &Path,
    home: &Path,
    installed_plugins: &std::collections::BTreeSet<String>,
) {
    let marketplace_name = marketplace
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(plugins) = marketplace.get_mut("plugins").and_then(Value::as_array_mut) else {
        return;
    };
    let marketplace_root = marketplace_path
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".tmp").join("plugins"));
    for plugin in plugins {
        let Some(plugin_object) = plugin.as_object_mut() else {
            continue;
        };
        let plugin_name = plugin_object
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                plugin_object
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| id.split('@').next())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if plugin_name.is_empty() {
            continue;
        }
        let manifest_path = marketplace_root
            .join("plugins")
            .join(&plugin_name)
            .join(".codex-plugin")
            .join("plugin.json");
        let plugin_root = marketplace_root.join("plugins").join(&plugin_name);
        if let Some(manifest) = plugin_manifest(&manifest_path) {
            merge_plugin_manifest(plugin_object, manifest);
        }
        apply_official_office_plugin_aliases(&marketplace_name, &plugin_name, plugin_object);
        absolutize_plugin_icon_paths(plugin_object, &plugin_root);
        plugin_object
            .entry("name".to_string())
            .or_insert_with(|| Value::String(plugin_name.clone()));
        plugin_object
            .entry("id".to_string())
            .or_insert_with(|| Value::String(format!("{plugin_name}@{marketplace_name}")));
        plugin_object
            .entry("marketplaceName".to_string())
            .or_insert_with(|| Value::String(marketplace_name.clone()));
        plugin_object
            .entry("marketplacePath".to_string())
            .or_insert_with(|| Value::String(marketplace_name.clone()));
        plugin_object
            .entry("keywords".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        plugin_object.insert(
            "installed".to_string(),
            Value::Bool(installed_plugins.contains(&format!("{plugin_name}@{marketplace_name}"))),
        );
    }
}

fn apply_official_office_plugin_aliases(
    marketplace_name: &str,
    plugin_name: &str,
    plugin: &mut Map<String, Value>,
) {
    if marketplace_name != "openai-primary-runtime" {
        return;
    }
    let display_name = match plugin_name {
        "documents" => "Word / Documents",
        "presentations" => "PPT / Presentations",
        "spreadsheets" => "Excel / Spreadsheets",
        _ => return,
    };
    let interface = plugin
        .entry("interface".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(interface) = interface.as_object_mut() {
        interface.insert(
            "displayName".to_string(),
            Value::String(display_name.to_string()),
        );
    }
}

fn absolutize_plugin_icon_paths(plugin: &mut Map<String, Value>, plugin_root: &Path) {
    for key in ["composerIconPath", "logoPath"] {
        absolutize_string_field(plugin, key, plugin_root);
    }
    let Some(interface) = plugin.get_mut("interface").and_then(Value::as_object_mut) else {
        return;
    };
    for key in ["composerIcon", "composerIconUrl", "logo", "logoUrl"] {
        absolutize_string_field(interface, key, plugin_root);
    }
}

fn absolutize_string_field(object: &mut Map<String, Value>, key: &str, root: &Path) {
    let Some(value) = object.get(key).and_then(Value::as_str).map(str::to_string) else {
        return;
    };
    let Some(path) = absolutize_plugin_asset_path(&value, root) else {
        return;
    };
    object.insert(key.to_string(), Value::String(path));
}

fn absolutize_plugin_asset_path(value: &str, root: &Path) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("data:")
        || trimmed.starts_with("http:")
        || trimmed.starts_with("https:")
        || trimmed.starts_with("file:")
        || Path::new(trimmed).is_absolute()
    {
        return None;
    }
    let relative = trimmed.strip_prefix("./").unwrap_or(trimmed);
    Some(root.join(relative).to_string_lossy().to_string())
}

fn plugin_manifest(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .as_object()
        .cloned()
}

fn merge_plugin_manifest(plugin: &mut Map<String, Value>, manifest: Map<String, Value>) {
    for (key, value) in manifest {
        plugin.entry(key).or_insert(value);
    }
}

fn installed_plugins_from_config(home: &Path) -> std::collections::BTreeSet<String> {
    let text = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    let doc = text.parse::<toml_edit::DocumentMut>().ok();
    let Some(plugins) = doc
        .as_ref()
        .and_then(|doc| doc.get("plugins"))
        .and_then(toml_edit::Item::as_table)
    else {
        return std::collections::BTreeSet::new();
    };
    plugins
        .iter()
        .filter_map(|(id, item)| {
            let enabled = item
                .get("enabled")
                .and_then(toml_edit::Item::as_bool)
                .unwrap_or(false);
            enabled.then(|| id.to_string())
        })
        .collect()
}

pub fn image_overlay_config(helper_port: u16, settings: &BackendSettings) -> Value {
    let has_path = !settings.codex_app_image_overlay_path.trim().is_empty();
    let enabled = settings.codex_app_image_overlay_enabled && has_path;
    let data_url = if enabled {
        image_file_data_uri(Path::new(settings.codex_app_image_overlay_path.trim()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    json!({
        "enabled": enabled && !data_url.is_empty(),
        "opacity": f64::from(settings.codex_app_image_overlay_opacity.clamp(1, 100)) / 100.0,
        "fitMode": settings.codex_app_image_overlay_fit_mode.as_str(),
        "dataUrl": data_url,
        "imageUrl": if enabled && !data_url.is_empty() {
            format!("http://127.0.0.1:{helper_port}/overlay/image")
        } else {
            String::new()
        },
    })
}

pub fn paste_fix_enabled_config(settings: &BackendSettings) -> Value {
    json!({ "enabled": settings.codex_app_paste_fix })
}

fn image_data_uri(mime_type: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn image_file_data_uri(path: &Path) -> Option<String> {
    let mime_type = image_content_type(path)?;
    let bytes = std::fs::read(path).ok()?;
    Some(image_data_uri(mime_type, &bytes))
}

fn image_content_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        Some("bmp") => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_fast_renderer_waits_for_dom_and_reinjection_runs_immediately() {
        let wrapped = renderer_after_document_ready("window.started=(window.started||0)+1;");
        let test = format!(
            r#"
          const vm=require('node:vm'),assert=require('node:assert/strict');
          for(const ready of [false,true]){{
            let listener;
            const window={{}},document={{body:ready?{{}}:null,addEventListener:(name,callback,options)=>{{
              assert.equal(name,'DOMContentLoaded');assert.equal(options.once,true);listener=callback;
            }}}};
            vm.runInNewContext({},{{window,document}});
            assert.equal(window.started,ready?1:undefined);
            if(!ready){{document.body={{}};listener();assert.equal(window.started,1);}}
          }}
        "#,
            serde_json::to_string(&wrapped).unwrap()
        );
        let output = std::process::Command::new("node")
            .arg("-e")
            .arg(test)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn native_shell_compatibility_is_present_without_managed_safe_css() {
        let temp = tempfile::tempdir().unwrap();
        for image in [
            String::new(),
            temp.path()
                .join("selected.png")
                .to_string_lossy()
                .into_owned(),
        ] {
            let mut settings = BackendSettings::default();
            settings.codex_app_dream_skin_enabled = true;
            settings.codex_app_dream_skin_image_path = image;
            assert!(managed_dream_skin_css(&settings).is_empty());
            let script = dream_skin_target_runtime_script(&settings, false);
            assert!(script.contains("全尺寸 shell"));
            settings.codex_app_dream_skin_paused = true;
            assert!(dream_skin_target_runtime_script(&settings, false).is_empty());
        }
    }

    #[test]
    fn injection_preserves_local_plugin_marketplace_cache_projection() {
        let script = injection_script_with_settings(57321, &BackendSettings::default());
        let prefix = "window.__CODEX_PLUS_PLUGIN_MARKETPLACES__ = ";
        assert_eq!(script.matches(prefix).count(), 1);
        let value = script
            .split_once(prefix)
            .expect("local plugin cache projection")
            .1
            .lines()
            .next()
            .unwrap();
        let marketplaces: Value = serde_json::from_str(value.trim_end_matches(';')).unwrap();
        assert!(
            marketplaces.is_array(),
            "the renderer must receive a valid marketplace list even when empty"
        );
    }

    #[test]
    fn agent_pet_sounds_are_embedded_byte_for_byte() {
        let script = injection_script(57321);
        let prefix = "window.__CODEX_PLUS_AGENT_PET_SOUNDS__ = ";
        let value = script.split_once(prefix).unwrap().1.lines().next().unwrap();
        let sounds: Value = serde_json::from_str(value.trim_end_matches(';')).unwrap();
        assert_eq!(sounds.as_object().unwrap().len(), 2);
        let decode = |uri: &Value| {
            base64::engine::general_purpose::STANDARD
                .decode(
                    uri.as_str()
                        .unwrap()
                        .strip_prefix("data:audio/mpeg;base64,")
                        .unwrap(),
                )
                .unwrap()
        };
        let expected: [(&str, &[u8], &[u8]); 2] = [
            (
                "duck",
                include_bytes!("../../../assets/inject/agent-pet/sounds/Ya1.mp3"),
                include_bytes!("../../../assets/inject/agent-pet/sounds/Ya2.mp3"),
            ),
            (
                "fx1",
                include_bytes!("../../../assets/inject/agent-pet/sounds/D1.mp3"),
                include_bytes!("../../../assets/inject/agent-pet/sounds/D2.mp3"),
            ),
        ];
        for (name, press, release) in expected {
            assert!(!press.is_empty() && !release.is_empty());
            assert_eq!(decode(&sounds[name]["press"]), press, "{name} press");
            assert_eq!(decode(&sounds[name]["release"]), release, "{name} release");
        }
    }

    #[test]
    fn community_shell_css_only_changes_native_surface_paint_and_insets() {
        let css = DREAM_SKIN_COMMUNITY_COMPAT_CSS;
        assert!(css.contains("[data-codex-os=\"win32\"]"));
        assert!(css.contains("[data-dream-task-mode=\"full\"]"));
        assert!(css.contains("--app-shell-titlebar-height: 0px"));
        assert!(css.contains("[data-app-action-timeline-scroll]"));
        assert!(css.contains("[role=\"menu\"]"));
        for excluded in [
            "opacity:",
            "z-index:",
            "position:",
            "img",
            "canvas",
            "video",
        ] {
            assert!(
                !css.contains(excluded),
                "unexpected media/layer override: {excluded}"
            );
        }
    }

    #[test]
    fn wallpaper_shell_compatibility_is_gated_and_preserves_theme_colors() {
        let css = DREAM_SKIN_COMMUNITY_COMPAT_CSS;
        let wallpaper_rules = css.split("@layer theme {").nth(1).unwrap();
        for rule in wallpaper_rules.split('}').filter(|rule| rule.contains('{')) {
            let selector = rule.split('{').next().unwrap();
            for guard in [
                "[data-dream-skin=\"active\"]",
                "[data-dream-art-ready=\"true\"]",
                "[data-dream-art-wide=\"true\"]",
                "[data-dream-task-mode=\"ambient\"]",
                "[data-dream-task-mode=\"banner\"]",
                "[data-dream-task-mode=\"full\"]",
            ] {
                assert!(
                    selector.contains(guard),
                    "unguarded wallpaper rule: {selector}"
                );
            }
        }
        assert!(wallpaper_rules.contains("_PageSurfaceLayout_"));
        assert!(wallpaper_rules.contains("main[data-app-shell-main-surface]"));
        assert!(wallpaper_rules.contains("background-color: transparent !important"));
        assert!(wallpaper_rules.contains("rgb(var(--ds-panel-rgb) / .68)"));
        assert!(wallpaper_rules.contains("color: var(--ds-text) !important"));
        assert!(wallpaper_rules.contains("[data-app-shell-titlebar=\"true\"]"));
        assert!(wallpaper_rules.contains("#codex-plus-usage-statusline"));
        assert!(wallpaper_rules.contains(".codex-token-usage-badge"));
    }

    #[test]
    fn image_overlay_config_includes_fit_mode() {
        let settings = BackendSettings {
            codex_app_image_overlay_fit_mode: "fill".to_string(),
            ..BackendSettings::default()
        };
        let config = image_overlay_config(57321, &settings);

        assert_eq!(config["fitMode"].as_str(), Some("fill"));
    }

    #[test]
    fn image_overlay_config_clears_endpoint_when_asset_is_unavailable() {
        let settings = BackendSettings {
            codex_app_image_overlay_enabled: true,
            codex_app_image_overlay_path: "C:\\missing\\overlay.png".to_string(),
            ..BackendSettings::default()
        };
        let config = image_overlay_config(57321, &settings);

        assert_eq!(config["enabled"], false);
        assert_eq!(config["dataUrl"], "");
        assert_eq!(config["imageUrl"], "");
    }

    #[test]
    fn local_plugin_marketplaces_includes_primary_runtime_and_third_party_sources() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let runtime_marketplace_dir = home
            .join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join(".agents")
            .join("plugins");
        let runtime_plugin_dir = home
            .join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join("plugins")
            .join("documents");
        let third_party_marketplace_dir = home
            .join("plugins")
            .join("sources")
            .join("awesome-codex-plugins")
            .join(".agents")
            .join("plugins");
        let third_party_plugin_dir = home
            .join("plugins")
            .join("sources")
            .join("awesome-codex-plugins")
            .join("plugins")
            .join("productivity");
        std::fs::create_dir_all(&runtime_marketplace_dir).unwrap();
        std::fs::create_dir_all(runtime_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(&third_party_marketplace_dir).unwrap();
        std::fs::create_dir_all(third_party_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::write(
            runtime_marketplace_dir.join("marketplace.json"),
            r#"{"name":"openai-primary-runtime","plugins":[{"name":"documents"}]}"#,
        )
        .unwrap();
        std::fs::write(
            runtime_plugin_dir.join(".codex-plugin").join("plugin.json"),
            r#"{"interface":{"displayName":"Documents"}}"#,
        )
        .unwrap();
        std::fs::write(
            third_party_marketplace_dir.join("marketplace.json"),
            r#"{"name":"awesome-codex-plugins","plugins":[{"name":"productivity"}]}"#,
        )
        .unwrap();
        std::fs::write(
            third_party_plugin_dir
                .join(".codex-plugin")
                .join("plugin.json"),
            r#"{"interface":{"displayName":"Productivity"}}"#,
        )
        .unwrap();

        let marketplaces = local_plugin_marketplaces_from_home(home);
        let array = marketplaces.as_array().unwrap();
        let names = array
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert!(names.contains(&"openai-primary-runtime"));
        assert!(names.contains(&"awesome-codex-plugins"));
        let runtime = array
            .iter()
            .find(|item| item["name"].as_str() == Some("openai-primary-runtime"))
            .unwrap();
        assert_eq!(
            runtime["plugins"][0]["interface"]["displayName"].as_str(),
            Some("Word / Documents")
        );
        let third_party = array
            .iter()
            .find(|item| item["name"].as_str() == Some("awesome-codex-plugins"))
            .unwrap();
        assert_eq!(
            third_party["plugins"][0]["interface"]["displayName"].as_str(),
            Some("Productivity")
        );
    }

    #[test]
    fn local_plugin_marketplaces_expose_official_office_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let runtime_marketplace_dir = home
            .join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join(".agents")
            .join("plugins");
        let documents_plugin_dir = home
            .join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join("plugins")
            .join("documents");
        let presentations_plugin_dir = home
            .join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join("plugins")
            .join("presentations");
        let spreadsheets_plugin_dir = home
            .join("plugins")
            .join("sources")
            .join("openai-primary-runtime")
            .join("plugins")
            .join("spreadsheets");
        std::fs::create_dir_all(&runtime_marketplace_dir).unwrap();
        std::fs::create_dir_all(documents_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(presentations_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(spreadsheets_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::write(
            runtime_marketplace_dir.join("marketplace.json"),
            concat!(
                r#"{"name":"openai-primary-runtime","plugins":["#,
                r#"{"name":"documents"},"#,
                r#"{"name":"presentations"},"#,
                r#"{"name":"spreadsheets"}"#,
                r#"]}"#
            ),
        )
        .unwrap();
        std::fs::write(
            documents_plugin_dir
                .join(".codex-plugin")
                .join("plugin.json"),
            r#"{"interface":{"displayName":"Documents"}}"#,
        )
        .unwrap();
        std::fs::write(
            presentations_plugin_dir
                .join(".codex-plugin")
                .join("plugin.json"),
            r#"{"interface":{"displayName":"Presentations"}}"#,
        )
        .unwrap();
        std::fs::write(
            spreadsheets_plugin_dir
                .join(".codex-plugin")
                .join("plugin.json"),
            r#"{"interface":{"displayName":"Spreadsheets"}}"#,
        )
        .unwrap();

        let marketplaces = local_plugin_marketplaces_from_home(home);
        let runtime = marketplaces
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"].as_str() == Some("openai-primary-runtime"))
            .unwrap();
        let plugins = runtime["plugins"].as_array().unwrap();

        assert_eq!(
            plugins[0]["interface"]["displayName"].as_str(),
            Some("Word / Documents")
        );
        assert_eq!(
            plugins[1]["interface"]["displayName"].as_str(),
            Some("PPT / Presentations")
        );
        assert_eq!(
            plugins[2]["interface"]["displayName"].as_str(),
            Some("Excel / Spreadsheets")
        );
    }

    #[test]
    fn local_plugin_marketplaces_include_tmp_marketplaces_and_skip_echobird_mirror() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let awesome_marketplace_dir = home
            .join(".tmp")
            .join("marketplaces")
            .join("awesome-codex-plugins")
            .join(".agents")
            .join("plugins");
        let awesome_plugin_dir = home
            .join(".tmp")
            .join("marketplaces")
            .join("awesome-codex-plugins")
            .join("plugins")
            .join("agent-workflow-system");
        let echobird_cn_marketplace_dir = home
            .join(".tmp")
            .join("marketplaces")
            .join("echobird-cn")
            .join(".agents")
            .join("plugins");
        let echobird_cn_plugin_dir = home
            .join(".tmp")
            .join("marketplaces")
            .join("echobird-cn")
            .join("plugins")
            .join("gmail");
        let echobird_ai_marketplace_dir = home
            .join(".tmp")
            .join("marketplaces")
            .join("echobird-ai")
            .join(".agents")
            .join("plugins");
        std::fs::create_dir_all(awesome_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(echobird_cn_plugin_dir.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(&awesome_marketplace_dir).unwrap();
        std::fs::create_dir_all(&echobird_cn_marketplace_dir).unwrap();
        std::fs::create_dir_all(&echobird_ai_marketplace_dir).unwrap();
        std::fs::write(
            awesome_marketplace_dir.join("marketplace.json"),
            r#"{"name":"awesome-codex-plugins","plugins":[{"name":"agent-workflow-system"}]}"#,
        )
        .unwrap();
        std::fs::write(
            awesome_plugin_dir.join(".codex-plugin").join("plugin.json"),
            r#"{"interface":{"displayName":"Agent Workflow System"}}"#,
        )
        .unwrap();
        std::fs::write(
            echobird_cn_marketplace_dir.join("marketplace.json"),
            r#"{"name":"echobird-cn","plugins":[{"name":"gmail"}]}"#,
        )
        .unwrap();
        std::fs::write(
            echobird_cn_plugin_dir
                .join(".codex-plugin")
                .join("plugin.json"),
            r#"{"interface":{"displayName":"Gmail"}}"#,
        )
        .unwrap();
        std::fs::write(
            echobird_ai_marketplace_dir.join("marketplace.json"),
            r#"{"name":"echobird-ai","plugins":[{"name":"gmail"}]}"#,
        )
        .unwrap();

        let marketplaces = local_plugin_marketplaces_from_home(home);
        let array = marketplaces.as_array().unwrap();
        let names = array
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert!(names.contains(&"awesome-codex-plugins"));
        assert!(names.contains(&"echobird-cn"));
        assert!(!names.contains(&"echobird-ai"));
    }
}

pub fn fast_startup_config(settings: &BackendSettings) -> Value {
    json!({ "enabled": settings.codex_app_fast_startup, "statsigTimeoutMs": 800 })
}
