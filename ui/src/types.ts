export interface Config {
  buffer_duration_secs: number;
  save_duration_secs: number;
  bitrate_kbps: number;
  fps: number;
  output_directory: string;
  output_prefix: string;
  hotkey: string;
  resolution: [number, number];
  capture_source: string | null;
  encoder: string | null;
  audio_enabled: boolean;
  audio_bitrate_kbps: number;
}

export interface BufferStatus {
  packetCount: number;
  totalBytes: number;
  maxBytes: number;
  durationSecs: number;
  utilizationPercent: number;
  isCapturing: boolean;
  isSaving: boolean;
}

export interface EncoderInfo {
  name: string;
  available: boolean;
}
