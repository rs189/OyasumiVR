import { Injectable } from '@angular/core';
import { IPCService } from '../ipc.service';
import { filter, map, switchMap, take } from 'rxjs';
import { Empty } from '../../../../src-grpc-web-client/overlay-sidecar_pb';
import { AppSettingsService } from '../app-settings.service';
import { APP_SETTINGS_DEFAULT, AppSettings } from '../../models/settings';

import { invoke } from '@tauri-apps/api/core';
import { VRChatService } from '../vrchat-api/vrchat.service';

@Injectable({
  providedIn: 'root',
})
export class OverlayService {
  public readonly sidecarStarted = this.ipcService.overlaySidecarClient.pipe(map(Boolean));
  private appSettings: AppSettings = structuredClone(APP_SETTINGS_DEFAULT);
  constructor(
    private ipcService: IPCService,
    private appSettingsService: AppSettingsService,
    private vrchat: VRChatService
  ) {}

  async init() {
    var gpu=this.appSettings.overlayGpuAcceleration;
    this.appSettingsService.settings.pipe(
      map((config)=>config.overlayGpuAcceleration)
    ).subscribe((enabled)=>{
      if (gpu!=enabled){
        gpu=enabled;
      }
      this.startOrRestartSidecar(enabled);
    });
    this.appSettingsService.settings.pipe(
      map((config)=>config.overlayMenuEnabled)
    ).subscribe((enabled)=>{
      if (!enabled){
        this.kill();
      }else{
        this.startOrRestartSidecar(this.appSettings.overlayGpuAcceleration);
      }
    });
    // Start the sidecar on launch
    this.appSettingsService.settings
      .pipe(
        take(1),
        filter((config) => config.overlayMenuEnabled),
        map((config) => config.overlayGpuAcceleration),
        switchMap((gpuAcceleration) => this.startOrRestartSidecar(gpuAcceleration))
      )
      .subscribe();
    // Respond to VRChat process state changes
    this.vrchat.vrchatProcessActive.subscribe((active) => {
      // Close the overlay menu if it's open and VRChat is no longer active
      if (!active && this.appSettings.overlayMenuOnlyOpenWhenVRChatIsRunning) {
        this.ipcService.getOverlaySidecarClient()?.closeOverlayMenu({} as Empty);
      }
    });
  }

  private async startOrRestartSidecar(gpuAcceleration: boolean) {
      await invoke('start_overlay_sidecar', { gpuAcceleration });
  }
  private async kill() {
      await invoke('stop_overlay_sidecar');
  }
}
