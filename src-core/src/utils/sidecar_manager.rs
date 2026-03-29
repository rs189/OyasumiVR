use log::{error, info, warn};
use oyasumi_shared::RESOURCES_PATH;
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::time::Duration;
// use sysinfo::{Pid, ProcessRefreshKind, System};
use tokio::sync::{Mutex, mpsc};

use crate::vr::openxr::OXR_HANDLE;
const LAUNCH_RETRY_INTERVALS: [Duration; 9] = [
    Duration::from_millis(100),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(300),
];

#[readonly::make]
pub struct SidecarManager {
    pub sidecar_id: String,
    pub exe_file: String,
    pub exe_dir: String,
    pub grpc_port: Arc<Mutex<Option<u32>>>,
    pub grpc_web_port: Arc<Mutex<Option<u32>>>,
    pub active: UnsafeCell<bool>,
    pub started: Arc<Mutex<bool>>,
    pub sidecar_child: Arc<Mutex<Option<std::process::Child>>>,
    pub on_stop_tx: mpsc::Sender<()>,
    pub auto_restart: bool,
    pub args: Arc<Mutex<Vec<String>>>,
    pub killed: Mutex<bool>,
}
unsafe impl Send for SidecarManager {}
unsafe impl Sync for SidecarManager {}
impl SidecarManager {
    pub fn new(
        sidecar_id: String,
        exe_dir: String,
        exe_file: String,
        on_stop_tx: mpsc::Sender<()>,
        auto_restart: bool,
        args: Vec<String>,
    ) -> Self {
        Self {
            sidecar_id,
            exe_file,
            exe_dir,
            grpc_port: Arc::new(Mutex::new(None)),
            grpc_web_port: Arc::new(Mutex::new(None)),
            active: UnsafeCell::new(false),
            started: Arc::new(Mutex::new(false)),
            sidecar_child: Arc::new(Mutex::new(None)),
            on_stop_tx,
            auto_restart,
            args: Arc::new(Mutex::new(args)),
            killed: Mutex::default(),
        }
    }

    pub async fn set_arg(&mut self, arg: &str, value: bool, unique: bool) {
        let mut args_guard = self.args.lock().await;
        let arg = String::from(arg);
        if value {
            if !unique || !args_guard.contains(&arg) {
                args_guard.push(arg);
            }
        } else if let Some(index) = args_guard.iter().position(|x| *x == arg) {
            args_guard.remove(index);
        }
    }

    #[allow(dead_code)]
    pub async fn set_args(&mut self, args: Vec<String>) {
        *self.args.lock().await = args;
    }

    pub async fn start_or_restart(&mut self) {
        // Kill process if it is already active
        if *self.active.get_mut() {
            info!(
                "[Core] Killing running {} sidecar to prepare for restart...",
                self.sidecar_id
            );
            let mut sidecar_child = self.sidecar_child.lock().await;
            if let Some(sidecar_child) = sidecar_child.as_mut() {
                if let Err(e) = sidecar_child.kill() {
                    error!("[Core] Failed to kill {} sidecar: {}", self.sidecar_id, e);
                }
            }else {
                warn!("sidecar child empty");
            }
        }
        // Start the process if it was not already running, or if auto_restart is not set
        if !*self.active.get_mut() || !self.auto_restart {
            self._start_internal(false).await;
        }
    }
    #[allow(dead_code)]
    pub async fn start(&mut self) -> u32 {
        self._start_internal(false).await
    }
    pub async fn stop(&mut self) {
        info!("stopping overlay sidecar");
        if let Some(overlay) = self.sidecar_child.lock().await.as_mut() {
            *self.killed.lock().await = true;
            let _ = overlay.kill();
        } else {
            warn!("tried to stop overlay but it's not running");
        }
    }

    async fn _start_internal(&mut self, relaunch: bool) -> u32 {
        let core_grpc_port_guard = crate::grpc::SERVER_PORT.lock().await;
        let core_grpc_port = match core_grpc_port_guard.as_ref() {
            Some(port) => *port,
            None => return 0,
        };
        if !relaunch && *self.active.get_mut() {
            return 0;
        }
        *self.active.get_mut() = true;
        info!(
            "[Core] {} {} sidecar...",
            match relaunch {
                true => "Restarting",
                false => "Starting",
            },
            self.sidecar_id
        );
        let exe_file = self.exe_file.clone();
        let mut exe_dir = std::path::PathBuf::from(self.exe_dir.as_str());
        let mut exe_path = std::path::Path::new(&exe_dir).join(&exe_file);
        if !exe_path.is_file() {
            error!(
                "[Core] {} sidecar not found at path:{:?}",
                self.sidecar_id, exe_path
            );
            return 0;
        }
        let mut args = vec![
            format!("{core_grpc_port}"),
            format!("{}", std::process::id()),
        ];
        {
            let extra_args = self.args.lock().await;
            for arg in extra_args.iter() {
                args.push(arg.clone());
            }
        }
        let child = {
            let cef_path = RESOURCES_PATH.join("sidecars/cef");
            if !cef_path.exists() {
                error!("cef path doesn't exist");
                return 0;
            }
            let cef_path = fs::canonicalize(cef_path).unwrap();
            use std::fs;

            exe_path = fs::canonicalize(exe_path).unwrap();
            exe_dir = fs::canonicalize(exe_dir).unwrap();
            std::process::Command::new(exe_path)
                .env("CEF_PATH", cef_path.clone().to_str().unwrap())
                .env("LD_LIBRARY_PATH", cef_path.clone().to_str().unwrap())
                .current_dir(&exe_dir)
                .args(&args)
                .spawn()
                .unwrap_or_else(|err| {
                    panic!(
                        "Could not spawn command {:?} {:?}, in path:{:?},with args:{:?}",
                        err, exe_file, exe_dir, args
                    )
                })
        };
        let child_pid = child.id();
        *self.sidecar_child.lock().await = Some(child);
        let self_ = unsafe { &mut *(&raw mut *self) };
        tokio::task::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let mut guard = self_.sidecar_child.lock().await;
                if let Some(child) = &mut *guard {
                    //process exit code should be collected
                    if let Some(exit_code) = child.try_wait().unwrap() {
                        log::info!("overlay sidecar exited with: {:?}", exit_code);
                        unsafe {
                            *self_.active.get() = false;
                        }
                        drop(guard);
                        if let Some(xr) = OXR_HANDLE.get() {
                            xr.lock().await.run();
                        }
                        break;
                    }
                }
            }
        });
        if !relaunch {
            self.watch_process();
        }
        child_pid
    }

    // The sidecar process is running
    #[allow(dead_code)]
    pub async fn is_active(&self) -> bool {
        unsafe { *self.active.get() }
    }
    #[allow(dead_code)]
    // The sidecar process is running, and the sidecar has signalled it has started
    pub async fn has_started(&self) -> bool {
        *self.started.lock().await
    }

    pub async fn handle_start_signal(
        &self,
        grpc_port: Option<u32>,
        grpc_web_port: Option<u32>,
        pid: u32,
        old_pid: Option<u32>,
    ) -> bool {
        // pid == 0 means that we are assuming the sidecar is running in development mode.
        if pid != 0 {
            // If the sidecar is not active, ignore this signal
            if !unsafe { *self.active.get() } {
                warn!(
                    "Ignoring start signal for {} sidecar with pid {} because it is not active",
                    self.sidecar_id, pid
                );
                return false;
            }
            // If another sidecar is already running that does not have the old pid, ignore this signal
            let bind = self.sidecar_child.lock().await;
            let current_pid = bind.as_ref();
            // let current_pid = self.sidecar_pid.lock().await;?
            if current_pid.is_some()
                && (current_pid.unwrap().id() != pid
                    && (old_pid.is_some() && current_pid.unwrap().id() != old_pid.unwrap()))
            {
                warn!(
                    "Ignoring start signal for {} sidecar with pid {} because another {} sidecar is already running with pid {}",
                    self.sidecar_id,
                    pid,
                    self.sidecar_id,
                    current_pid.unwrap().id()
                );
                return false;
            }
        } else {
            // We already expect it to run in development mode
            unsafe {
                *self.active.get() = true;
            }
        }
        // Store started state
        *self.started.lock().await = true;
        // Store the GRPC ports
        *self.grpc_port.lock().await = grpc_port;
        *self.grpc_web_port.lock().await = grpc_web_port;
        info!(
            "[Core] Detected start of {} sidecar (pid={}, grpc_port={:?}, grpc_web_port={:?})",
            self.sidecar_id, pid, grpc_port, grpc_web_port
        );
        true
    }

    fn watch_process(&mut self) {
        // let mut s = System::new();
        let self_ = unsafe { &mut *(&raw mut *self) };
        tokio::spawn(async move {
            let mut retries = 0;
            loop {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if !*self_.active.get_mut() {
                        let sidecar_child = &self_.sidecar_child;
                        *sidecar_child.lock().await = None;
                        let grpc_port = &self_.grpc_port;
                        *grpc_port.lock().await = None;
                        let grpc_web_port = &self_.grpc_web_port;
                        *grpc_web_port.lock().await = None;
                        *self_.active.get_mut() = false;
                        let started = &self_.started;
                        *started.lock().await = false;
                        // }
                        // Send signal that the sidecar has stopped
                        let _ = &self_.on_stop_tx.send(());
                        info!("[Core] {} sidecar has stopped", &self_.sidecar_id);
                        break;
                    } else {
                        retries = 0;
                    }
                }
                // Automatically try restarting the sidecar if desired
                if self_.auto_restart {
                    if *self_.killed.lock().await {
                        *self_.killed.lock().await = false;
                        *self_.active.get_mut() = false;
                        break;
                    }
                    let retry_interval = LAUNCH_RETRY_INTERVALS[retries];
                    tokio::time::sleep(retry_interval).await;
                    retries += 1;
                    if retries >= LAUNCH_RETRY_INTERVALS.len() {
                        retries = LAUNCH_RETRY_INTERVALS.len() - 1;
                    }
                    // KICKSTART THE SIDECAR
                    self_._start_internal(true).await;
                }
            }
        });
    }
}
