//! WASAPI shared-mode loopback capture of the default render endpoint.

use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

use crate::audio::{CHANNELS, SAMPLE_RATE};
use crate::{HostError, Result};

pub struct LoopbackCapture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

// COM pointers created in the MTA; used from a single dedicated thread.
unsafe impl Send for LoopbackCapture {}

impl LoopbackCapture {
    /// Initialises COM (MTA) on the calling thread; call from the thread that
    /// will later call `poll`.
    pub fn new() -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            // Ask for 48 kHz stereo f32 explicitly; AUTOCONVERTPCM lets the
            // audio engine resample/remix from the endpoint mix format.
            let fmt = WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: CHANNELS as u16,
                nSamplesPerSec: SAMPLE_RATE,
                nAvgBytesPerSec: SAMPLE_RATE * CHANNELS as u32 * 4,
                nBlockAlign: (CHANNELS * 4) as u16,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                1_000_000, // 100 ms buffer, in 100 ns units
                0,
                &fmt,
                None,
            )?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self { client, capture })
        }
    }

    /// Appends all currently available samples (interleaved f32) to `out`.
    /// Note: loopback delivers NO packets while nothing is playing; callers
    /// must tolerate gaps (or inject silence to keep a steady cadence).
    pub fn poll(&mut self, out: &mut Vec<f32>) -> Result<()> {
        unsafe {
            loop {
                let n = self.capture.GetNextPacketSize()?;
                if n == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let samples = frames as usize * CHANNELS;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    out.extend(std::iter::repeat(0.0f32).take(samples));
                } else {
                    out.extend_from_slice(std::slice::from_raw_parts(data as *const f32, samples));
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

#[allow(dead_code)]
fn _unused(_: HostError) {}
