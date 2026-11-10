mod credentials;
mod smoke;
pub use smoke::main as smoke_main;

use crate::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter, Manager};
use voicekey_core::VolcEngine;

pub struct State {
    pub engine: Arc<VolcEngine>,
    operation: tokio::sync::Mutex<()>,
    loading: AtomicBool,
    error: RwLock<Option<String>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            engine: Arc::new(VolcEngine::default()),
            operation: tokio::sync::Mutex::new(()),
            loading: AtomicBool::new(true),
            error: RwLock::new(None),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Status {
    configured: bool,
    loading: bool,
    error: Option<String>,
}

impl State {
    pub fn status(&self) -> Status {
        Status {
            configured: self.engine.configured(),
            loading: self.loading.load(Ordering::Acquire),
            error: self.error.read().ok().and_then(|error| error.clone()),
        }
    }

    fn finish(&self, app: &AppHandle, result: &Result<(), String>) {
        *self.error.write().unwrap() = result.as_ref().err().cloned();
        self.loading.store(false, Ordering::Release);
        let _ = app.emit("official-status", self.status());
    }
}

pub fn initialize(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>().official.clone();
        let _guard = state.operation.lock().await;
        let result = match tauri::async_runtime::spawn_blocking(credentials::load).await {
            Ok(Ok(Some(key))) => state.engine.set_key(key).map_err(|error| error.to_string()),
            Ok(Ok(None)) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("读取火山密钥任务失败，请重启应用".into()),
        };
        state.finish(&app, &result);
    });
}

#[tauri::command]
pub async fn official_save_key(app: AppHandle, key: String) -> Result<Status, String> {
    let key = voicekey_core::volc::validate_key(&key).map_err(|error| error.to_string())?;
    let state = app.state::<AppState>().official.clone();
    let _guard = state.operation.lock().await;
    let saved = key.clone();
    let result = match tauri::async_runtime::spawn_blocking(move || credentials::save(&saved)).await
    {
        Ok(Ok(())) => state.engine.set_key(key).map_err(|error| error.to_string()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("保存火山密钥任务失败，请重试".into()),
    };
    state.finish(&app, &result);
    result.map(|_| state.status())
}

#[tauri::command]
pub async fn official_clear_key(app: AppHandle) -> Result<Status, String> {
    let state = app.state::<AppState>().official.clone();
    let _guard = state.operation.lock().await;
    let result = match tauri::async_runtime::spawn_blocking(credentials::clear).await {
        Ok(Ok(())) => {
            state.engine.clear_key();
            Ok(())
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err("移除火山密钥任务失败，请重试".into()),
    };
    state.finish(&app, &result);
    result.map(|_| state.status())
}
