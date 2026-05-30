use std::time::{SystemTime, UNIX_EPOCH};

pub fn info(target: &str, message: impl AsRef<str>) {
    log("INFO", target, message.as_ref());
}

pub fn warn(target: &str, message: impl AsRef<str>) {
    log("WARN", target, message.as_ref());
}

pub fn error(target: &str, message: impl AsRef<str>) {
    log("ERROR", target, message.as_ref());
}

fn log(level: &str, target: &str, message: &str) {
    let (seconds, millis) = timestamp();
    eprintln!("{seconds}.{millis:03} level={level} target={target} {message}");
}

fn timestamp() -> (u64, u32) {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (duration.as_secs(), duration.subsec_millis())
}
