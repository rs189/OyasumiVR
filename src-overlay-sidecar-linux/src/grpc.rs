use std::{net::SocketAddr, time::Duration};

use log::{error, info, trace};
use tonic::transport::Server;
use tonic_web::GrpcWebLayer;
use tower_http::cors::{AllowHeaders, AllowOrigin};
use xr_overlay::{
    openxr::{Posef, Quaternionf, Vector3f},
    runner::DeviceRole,
};

use crate::{
    ARGS, HANDLES,
    globals::STATE,
    overlay_grpc::{
        AddNotificationRequest, AddNotificationResponse, ClearNotificationRequest, Empty,
        OverlayMenuOpenRequest, OyasumiSidecarState, SetDebugTranslationsRequest,
        SetMicrophoneActiveRequest,
        oyasumi_overlay_sidecar_server::{OyasumiOverlaySidecar, OyasumiOverlaySidecarServer},
    },
    overlay_ipc::OverlayIPCAddNotification,
    vr::{
        DASBOARD_VISIBLE, NOTIFICATION_OVERLAY, OVERLAY, XR_CTX, hide_dashboard, set_mic_active,
        show_dashboard,
    },
};
#[derive(Debug, Default, Clone)]
pub struct GrpcServer {}
#[allow(unused_variables)]
#[tonic::async_trait]
impl OyasumiOverlaySidecar for GrpcServer {
    async fn add_notification(
        &self,
        request: tonic::Request<AddNotificationRequest>,
    ) -> Result<tonic::Response<AddNotificationResponse>, tonic::Status> {
        let req = request.into_inner();
        let id = NOTIFICATION_OVERLAY
            .wait()
            .add_notification(OverlayIPCAddNotification {
                message: &req.message,
                duration: Duration::from_millis(req.duration as u64),
            });
        Ok(AddNotificationResponse {
            notification_id: Some(id),
        }
        .into())
    }

    async fn clear_notification(
        &self,
        request: tonic::Request<ClearNotificationRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        NOTIFICATION_OVERLAY
            .wait()
            .clear_notification(&request.into_inner().notification_id);
        Ok(Empty {}.into())
    }

    async fn sync_state(
        &self,
        request: tonic::Request<OyasumiSidecarState>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        #[cfg(debug_assertions)]
        log::trace!("got new state from core:{:?}", request);
        let req = request.into_inner();
        STATE.lock().await.replace(req.clone());
        OVERLAY.wait().set_state(req);
        Ok(Empty::default().into())
    }

    async fn set_debug_translations(
        &self,
        request: tonic::Request<SetDebugTranslationsRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        todo!()
    }

    async fn open_overlay_menu(
        &self,
        request: tonic::Request<OverlayMenuOpenRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        trace!("open_overlay_menu");
        if let Some(ctx) = XR_CTX.get()
            && let Some(overlay) = OVERLAY.get()
        {
            ctx.write()
                .unwrap()
                .set_posef_relative(
                    DeviceRole::Hmd,
                    overlay.xr_handle,
                    Posef {
                        orientation: Quaternionf::IDENTITY,
                        position: Vector3f {
                            x: 0.0,
                            y: -0.1,
                            z: -0.8,
                        },
                    },
                    true,
                )
                .unwrap();
            show_dashboard();
        } else {
            log::warn!("vr not ready");
        }

        Ok(Empty {}.into())
    }

    async fn close_overlay_menu(
        &self,
        request: tonic::Request<Empty>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        trace!("close_overlay_menu");
        hide_dashboard().await;
        Ok(Empty {}.into())
    }

    async fn toggle_overlay_menu(
        &self,
        request: tonic::Request<OverlayMenuOpenRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        trace!("toggle_overlay_menu");
        match unsafe { DASBOARD_VISIBLE } {
            true => hide_dashboard().await,
            false => show_dashboard(),
        }
        Ok(Empty {}.into())
    }

    async fn set_microphone_active(
        &self,
        request: tonic::Request<SetMicrophoneActiveRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        let req = request.into_inner();
        if req.mode == 0 {
            // 0= hardware
            set_mic_active(req.active);
        } else {
            set_mic_active(false);
        }
        Ok(Empty {}.into())
    }
}
pub async fn start_grpc_server() -> u16 {
    let port: u16 = match ARGS.get().as_ref().unwrap().core_pid == 0 {
        true => crate::globals::OVERLAY_SIDECAR_GRPC_DEV_PORT,
        false => portpicker::pick_unused_port().unwrap(),
    };
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    info!("Starting gRPC server on {}", addr);

    let server = Server::builder()
        .add_service(OyasumiOverlaySidecarServer::new(GrpcServer::default()))
        .serve(addr);
    HANDLES.lock().unwrap().push(tokio::task::spawn(async move {
        {
            if let Err(err) = server.await {
                error!("Failed to start gRPC server: {}", err);
            }
        }
    }));
    addr.port()
}
pub async fn start_grpc_web_server() -> u16 {
    let port: u16 = match ARGS.get().as_ref().unwrap().core_pid == 0 {
        true => crate::globals::OVERLAY_SIDECAR_GRPC_WEB_DEV_PORT,
        false => portpicker::pick_unused_port().unwrap(),
    };
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    info!("Starting gRPC web server on {}", addr);
    let server = Server::builder()
        .accept_http1(true)
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(AllowOrigin::any())
                .allow_headers(AllowHeaders::any()),
        )
        .layer(GrpcWebLayer::new())
        .add_service(OyasumiOverlaySidecarServer::new(GrpcServer::default()))
        .serve(addr);
    HANDLES.lock().unwrap().push(tokio::task::spawn(async move {
        {
            if let Err(err) = server.await {
                error!("Failed to start web gRPC server: {}", err);
            }
        }
    }));
    addr.port()
}
