use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{Datelike, Local, Timelike};

unsafe impl Send for Writter {}
unsafe impl Sync for Writter {}
#[derive(Default)]
pub struct Writter {
    pub file: Arc<Mutex<Option<File>>>,
}
impl Write for Writter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(file) = self.file.lock().unwrap().as_mut() {
            std::io::stdout().write(buf)?;
            file.write(buf)
        } else {
            std::io::stdout().write(buf)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = self.file.lock().unwrap().as_mut() {
            std::io::stdout().flush()?;
            file.flush()
        } else {
            std::io::stdout().flush()
        }
    }
}
pub fn get_o_log_path() -> PathBuf {
    let path = oyasumi_shared::get_log_path().join("Overlay.log");
    if path.exists() {
        let c_d = path
            .metadata()
            .unwrap()
            .created()
            .unwrap_or(SystemTime::now());
        let c_d = c_d.duration_since(UNIX_EPOCH).unwrap();
        let c_d = chrono::DateTime::from_timestamp(c_d.as_secs() as i64, c_d.subsec_nanos())
            .unwrap()
            .with_timezone(&Local);
        fs::rename(
            &path,
            oyasumi_shared::get_log_path().join(format!(
                "Overlay_{}-{}-{}_{}-{}-{}.log",
                c_d.year(),
                c_d.month(),
                c_d.day(),
                c_d.hour(),
                c_d.minute(),
                c_d.second()
            )),
        )
        .expect("failed to rotate log file");
    }
    path
}
