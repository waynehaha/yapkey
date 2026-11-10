#[cfg(target_os = "macos")]
mod keychain {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    const SERVICE: &str = "do.j3.voicekey.volc-speech";
    const ACCOUNT: &str = "api-key";
    const NOT_FOUND: i32 = -25300;

    pub fn load() -> Result<Option<String>, String> {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "钥匙串中的火山密钥格式无效，请重新保存".into()),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(format!(
                "读取火山密钥失败，请检查钥匙串访问权限（{}）",
                error.code()
            )),
        }
    }

    pub fn save(key: &str) -> Result<(), String> {
        set_generic_password(SERVICE, ACCOUNT, key.as_bytes())
            .map_err(|error| format!("保存火山密钥到钥匙串失败（{}）", error.code()))
    }

    pub fn clear() -> Result<(), String> {
        match delete_generic_password(SERVICE, ACCOUNT) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(format!("移除火山密钥失败（{}）", error.code())),
        }
    }
}

#[cfg(target_os = "macos")]
pub use keychain::{clear, load, save};

#[cfg(not(target_os = "macos"))]
pub fn load() -> Result<Option<String>, String> {
    Ok(None)
}
#[cfg(not(target_os = "macos"))]
pub fn save(_: &str) -> Result<(), String> {
    Err("当前平台暂不支持火山密钥安全存储".into())
}
#[cfg(not(target_os = "macos"))]
pub fn clear() -> Result<(), String> {
    Ok(())
}
