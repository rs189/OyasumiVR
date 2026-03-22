use std::{
    fs,
    path::PathBuf,
    sync::{Arc, LazyLock, OnceLock, RwLock},
    thread::JoinHandle,
    time::Duration,
};

use log::{info, trace};
use oyasumi_shared::OVERLAY_CONFIG_PATH;
use xr_overlay::{
    openxr::Vector3f,
    runner::{
        AppRunner, AppRunnerCreateInfo, AppRunnerCreateInfoInput, DeviceRole, OverlayCreateInfo,
        OverlayHandle, ShowMode, events::AppEvent,
    },
};
use xr_overlay_cef::{
    CefOverlayCreateInfo,
    cef::{ImplBrowser, ImplFrame},
    create_cef_overlay,
};
pub static CACHE_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| PathBuf::from("/tmp/oyasumi_sidecard_cef"));
pub const DEFAULT_BINDINGS_CONFIG: &str = include_str!("../../bindings_config.toml");
use crate::{
    CONFIG, HTTP_PORT, KILL, UI_PORT, WS_PORT, config::OverlayConfig, globals::textures,
    input::get_controller_create_info, killed, model::Overlay,
};
pub static OVERLAY: OnceLock<Overlay> = OnceLock::new();
pub static NOTIFICATION_OVERLAY: OnceLock<Overlay> = OnceLock::new();
pub static MIC_MUTE_OVERLAY: OnceLock<OverlayHandle> = OnceLock::new();
pub static XR_CTX: OnceLock<Arc<RwLock<AppRunner>>> = OnceLock::new();
pub fn start_vr() -> Option<JoinHandle<()>> {
    if CACHE_PATH.exists() {
        log::info!("deleting /tmp cache:{:#?}", CACHE_PATH);
        fs::remove_dir_all(CACHE_PATH.clone()).unwrap();
    } else {
        info!("/tmp/ cache doesn't exist yet");
    }
    trace!("start_vr");
    let ctx = loop {
        if killed() {
            return None;
        }
        match xr_overlay::xr::Init::default()
            .enable_drm_support()
            .user_presence_support(true)
            .sort_order(4089)
            .with_app_name("Oyasumi VR Overlay")
            .init_overlay()
        {
            Ok(v) => break v,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(1));
            }
        };
    };

    let config = OverlayConfig::default_with_config(
        &fs::read_to_string(&*OVERLAY_CONFIG_PATH).unwrap(),
        ctx.xr.session.get_display_refresh_rate().unwrap() as u8,
    )
    .unwrap();
    CONFIG.set(config.clone()).unwrap();
    info!("parsed config: {:#?}", config);

    let app = AppRunner::new(AppRunnerCreateInfo {
        ctx,
        space_type: xr_overlay::xr::ReferenceSpaceT::STAGE,
        callback: openxr_callback,
        input: Some(AppRunnerCreateInfoInput {
            l_pointer_color: config.pointers.left_color,
            r_pointer_color: config.pointers.right_color,
            controllers: get_controller_create_info(),
            draw_pointers_only_when_hit: config.pointers.draw_only_when_on_overlay,
            overlay_move_speed: config.pointers.overlay_move_speed,
        }),
    });

    let app = Arc::new(RwLock::new(app));
    XR_CTX.set(app.clone()).unwrap();
    let pos = config.main_overlay.position;
    let notifica_pos = config.notification_overlay.position;
    let mic_pos = config.mute_indicator_overlay.position;

    let overlay = create_cef_overlay(
        app.clone(),
        CefOverlayCreateInfo {
            size: config.main_overlay.size.into(),
            spawn_visible: true,
            interactable: true,
            movable: true,
            allow_visibility_switch: true,
            pos: Vector3f {
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            framerate: config.main_overlay.framerate as u32,
            resolution: config.main_overlay.resolution,
            show_mode: config.main_overlay.show_mode,
            name: Some("oyasumi".into()),
            disable_dragging: true,
            reference_space: Some(config.main_overlay.reference_space),
            cache_path: Some(CACHE_PATH.clone()),
            ..Default::default()
        },
    );
    let noti_overlay = create_cef_overlay(
        app.clone(),
        CefOverlayCreateInfo {
            movable: false,
            interactable: false,
            size: config.notification_overlay.size.into(),
            spawn_visible: true,
            pos: Vector3f {
                x: notifica_pos[0],
                y: notifica_pos[1],
                z: notifica_pos[2],
            },
            framerate: config.notification_overlay.framerate as u32,
            resolution: config.notification_overlay.resolution,
            name: Some("notifications".into()),
            reference_space: Some(config.notification_overlay.reference_space),
            cache_path: Some(CACHE_PATH.clone()),
            ..Default::default()
        },
    );
    let mic_overlay = app.write().unwrap().add_overlay(OverlayCreateInfo {
        type_: xr_overlay::runner::OverlayCreateInfoType::Unmanaged {
            size: config.mute_indicator_overlay.size.into(),
        },
        pos: Vector3f {
            x: mic_pos[0],
            y: mic_pos[1],
            z: mic_pos[2],
        },
        spawn_visible: true,
        show_mode: ShowMode::default(),
        name: Some("mic_mute".to_owned()),
        reference_space: Some(config.mute_indicator_overlay.reference_space),
        allow_visibility_switch: false,
        movable: false,
        interactable: false,
        ..Default::default()
    });
    MIC_MUTE_OVERLAY.set(mic_overlay).unwrap();
    *MIC_INDICATOR.lock().unwrap() = Some(MicMuteIndicator::new(mic_overlay));
    let delay = Duration::from_millis(500).as_millis() as f32
        / (1000. / app.read().unwrap().current_refresh_rate() as f32);
    app.write()
        .unwrap()
        .set_delay_hide(overlay.overlay_handle, delay as u8);
    assert!(
        OVERLAY
            .set(Overlay {
                browser: overlay.browser.clone(),
                xr_handle: overlay.overlay_handle
            })
            .is_ok()
    );
    assert!(
        NOTIFICATION_OVERLAY
            .set(Overlay {
                browser: noti_overlay.browser.clone(),
                xr_handle: noti_overlay.overlay_handle
            })
            .is_ok()
    );

    Some(std::thread::spawn(move || {
        let frame_time = (1000. / app.write().unwrap().current_refresh_rate() as f32) as u64;
        loop {
            if unsafe { KILL } {
                break;
            }
            if let Some(indicator) = MIC_INDICATOR.lock().unwrap().as_mut() {
                indicator.update_frame();
            }
            let mut guard = app.write().unwrap();
            match guard.run(false) {
                xr_overlay::runner::PollResult::Success(_) => {
                    //it already waits for next frame
                    drop(guard);
                    // std::thread::sleep(v.saturating_sub(Duration::fr(700)));
                    // std::thread::sleep(Duration::from_millis(frame_time))
                }
                xr_overlay::runner::PollResult::SuccessNoRender => {
                    drop(guard);
                    std::thread::sleep(Duration::from_millis(frame_time))
                }
                xr_overlay::runner::PollResult::UserNotPresent => {
                    drop(guard);
                    std::thread::sleep(Duration::from_secs(1))
                }
                xr_overlay::runner::PollResult::Starting => (),
                xr_overlay::runner::PollResult::Exit => {
                    //no session resuming bc google's trash doesn't support restarting after calling shutdown
                    // unsafe { xr_overlay_cef::shutdown() };
                    break;
                }
                xr_overlay::runner::PollResult::SessionLost => {
                    break;
                }
            }
        }
    }))
}
fn openxr_callback(event: AppEvent) {
    if event != AppEvent::ButtonsUpdated {
        trace!("[openxr] {:?}", event);
    }
    match event {
        AppEvent::OverlayVisibilityChanged { handle: _, visible } => {
            if visible {
                OVERLAY.get().as_ref().unwrap().show_dashboard();
                log::info!("overlay showed");
            } else {
                log::info!("overlay hidden");
            }
        }
        AppEvent::OverlayHiding {
            handle: _,
            frames_left: frames,
        } => {
            trace!("hiding animation:{}", frames);
            OVERLAY.get().as_ref().unwrap().hide_dashboard()
        }
        AppEvent::OverlayVisibilityChangedLastInput {
            handle,
            visible,
            last_input,
        } => {
            if visible && handle == OVERLAY.wait().xr_handle {
                *SWITCH_HAND.lock().unwrap() = last_input.hand();
            }
        }
        _ => (),
    }
}
static SWITCH_HAND: std::sync::Mutex<xr_overlay::xr_input::Hand> =
    std::sync::Mutex::new(xr_overlay::xr_input::Hand::Left);
pub fn openxr_show_hand() -> DeviceRole {
    (*SWITCH_HAND.lock().unwrap()).into()
}
pub static mut DASBOARD_VISIBLE: bool = false;
pub static mut SPLASH_PLAYED: bool = false;
pub fn show_dashboard() {
    trace!("show_dashboard");
    unsafe { DASBOARD_VISIBLE = true };
    XR_CTX
        .wait()
        .write()
        .unwrap()
        .set_visible(OVERLAY.wait().xr_handle, true);
    OVERLAY.wait().show_dashboard();
}
pub async fn hide_dashboard() {
    trace!("hide_dashboard");
    unsafe { DASBOARD_VISIBLE = false };
    OVERLAY.wait().hide_dashboard();
    tokio::time::sleep(Duration::from_millis(500)).await; //give animation some time
    XR_CTX
        .wait()
        .write()
        .unwrap()
        .set_visible(OVERLAY.wait().xr_handle, false);
    if unsafe { SPLASH_PLAYED } {
        let url = format!(
            "http://localhost:{}/dashboard?corePort={}",
            UI_PORT.wait(),
            HTTP_PORT.wait()
        );
        // let url_noti="https://google.com".to_string();
        trace!("navigating to:{}", url);
        OVERLAY
            .get()
            .as_ref()
            .unwrap()
            .browser
            .main_frame()
            .unwrap()
            .load_url(Some(&(url.as_str()).into()));
        unsafe { SPLASH_PLAYED = false };
        tokio::time::sleep(Duration::from_millis(50)).await;
        OVERLAY.get().as_mut().unwrap().inject_ipc(*WS_PORT.wait());
    }
}
pub static MIC_INDICATOR: std::sync::Mutex<Option<MicMuteIndicator>> = std::sync::Mutex::new(None);

pub struct MicMuteIndicator {
    overlay_handle: OverlayHandle,
    mute_state: bool,
    mic_active: bool,
    max_opacity: f32,
    fade_out: bool,
    enabled: bool,
    last_state_change: std::time::Instant,
    last_presence_indication: std::time::Instant,
    last_mic_activity_change: std::time::Instant,
    last_set_scale: f32,
    base_scale: f32,
    mute_image_state: Option<bool>,
    last_opacity: f32,
}

impl MicMuteIndicator {
    pub fn new(handle: OverlayHandle) -> Self {
        Self {
            overlay_handle: handle,
            mute_state: true,
            mic_active: false,
            max_opacity: 100.0,
            fade_out: false,
            enabled: false,
            last_state_change: std::time::Instant::now(),
            last_presence_indication: std::time::Instant::now(),
            last_mic_activity_change: std::time::Instant::now(),
            last_set_scale: 0.0,
            base_scale: 0.088,
            mute_image_state: None,
            last_opacity: -1.0,
        }
    }

    pub fn set_active(&mut self, active: bool) {
        if self.mic_active != active {
            self.last_mic_activity_change = std::time::Instant::now();
        }
        self.mic_active = active;
    }

    pub fn update_frame(&mut self) {
        if let Some(state) = crate::globals::STATE.blocking_lock().as_ref() {
            if state.system_mic_muted != self.mute_state {
                self.mute_state = state.system_mic_muted;
                self.last_state_change = std::time::Instant::now();
            }
            if let Some(settings) = &state.settings {
                self.max_opacity = settings.system_mic_indicator_opacity as f32;
                self.fade_out = settings.system_mic_indicator_fadeout;
                if settings.system_mic_indicator_enabled != self.enabled {
                    self.enabled = settings.system_mic_indicator_enabled;
                    XR_CTX
                        .wait()
                        .write()
                        .unwrap()
                        .set_visible(self.overlay_handle, self.enabled);
                }
            }
        }
        if !self.enabled {
            return;
        }

        let now = std::time::Instant::now();
        let time_since_last_state_change =
            now.duration_since(self.last_state_change).as_millis() as f32;
        let time_since_last_presence_indication = now
            .duration_since(self.last_presence_indication)
            .as_millis() as f32;
        let time_since_last_mic_activity_change = now
            .duration_since(self.last_mic_activity_change)
            .as_millis() as f32;

        let max_opacity = self.max_opacity * if self.mute_state { 1.0 } else { 0.1 };

        let opacity = if self.mic_active && !self.mute_state {
            self.max_opacity / 100.
        } else if self.fade_out {
            let time_since = time_since_last_presence_indication
                .min(time_since_last_state_change)
                .min(time_since_last_mic_activity_change);
            // MathUtils.InvLerpClamped(3500, 6000, timeSince)
            let val = (time_since - 3500.) / (6000. - 3500.);
            let val = val.clamp(0., 1.);
            let opacity_factor = val * val; // InQuad
            (max_opacity / 100.) * (1.0 - opacity_factor)
        } else {
            max_opacity / 100.
        };

        if self.mute_image_state != Some(self.mute_state)
            || (self.last_opacity - opacity).abs() > 0.001
        {
            self.mute_image_state = Some(self.mute_state);
            self.last_opacity = opacity;

            let texture = match self.mute_state {
                true => textures::MIC_MUTE.clone(),
                false => textures::MIC_UNMUTE.clone(),
            };
            XR_CTX.wait().write().unwrap().set_raw_texture(
                self.overlay_handle,
                texture,
                opacity,
                false,
            );
        }

        // Scale
        let t_state = ((time_since_last_state_change - 200.) / (350. - 200.)).clamp(0., 1.);
        let scale_state_change_factor = t_state * t_state;

        let mut scale_activity_change_factor = 1.0;
        if !self.mute_state {
            let t_activity =
                ((time_since_last_mic_activity_change - 0.) / (100. - 0.)).clamp(0., 1.);
            let factor = t_activity * t_activity;
            if self.mic_active {
                scale_activity_change_factor = 1.0 - factor;
            }
        }
        let scale_factor = scale_state_change_factor.min(scale_activity_change_factor);
        let scale = ((1.0 - scale_factor) * 0.25 + 1.0) * self.base_scale;

        if (self.last_set_scale - scale).abs() > 0.001 {
            XR_CTX
                .wait()
                .write()
                .unwrap()
                .set_size(self.overlay_handle, [scale, scale].into());
            self.last_set_scale = scale;
        }
    }
}

pub fn set_mic_active(active: bool) {
    if let Some(indicator) = MIC_INDICATOR.lock().unwrap().as_mut() {
        indicator.set_active(active);
    }
}
