//! This computer's sound on Windows, through WASAPI's process loopback
//! (Windows 10 version 2004 and later): everything playing, but this
//! process and anything it started, so the call stays out.

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient,
    IAudioClient, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{HRESULT, IUnknown, Interface, Ref, implement};

use super::{Missing, Push};
use crate::core::voice::sound::RATE;

/// How long Windows gets to set it up.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);
/// WAVE_FORMAT_IEEE_FLOAT.
const FLOAT: u16 = 3;

pub struct Capture(Arc<AtomicBool>);

impl Drop for Capture {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub fn open(push: Push) -> Result<Capture, Missing> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready) = mpsc::channel();
    let stopping = stop.clone();
    std::thread::Builder::new()
        .name("fuwa-screen-sound".into())
        .spawn(move || {
            // SAFETY: COM for this thread alone, undone as it ends.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            match start() {
                Ok(started) => {
                    let _ = ready_tx.send(Ok(()));
                    run(&started, &stopping, &push);
                    // SAFETY: stopping the client `start` started, and closing its event.
                    unsafe {
                        let _ = started.client.Stop();
                        let _ = CloseHandle(started.event);
                    }
                }
                Err(missing) => {
                    let _ = ready_tx.send(Err(missing));
                }
            }
            // SAFETY: matches the CoInitializeEx above.
            unsafe { CoUninitialize() };
        })
        .map_err(|_| Missing::Failed)?;
    ready.recv_timeout(ANSWER_WITHIN).map_err(|_| Missing::Failed)??;
    Ok(Capture(stop))
}

/// Says when Windows has activated the loopback client.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(mpsc::Sender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

struct Started {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
}

fn start() -> Result<Started, Missing> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                // SAFETY: always succeeds.
                TargetProcessId: unsafe { GetCurrentProcessId() },
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let blob = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw mut params).cast(),
                    },
                },
            }),
        },
    };
    let (done_tx, done) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(done_tx).into();
    // SAFETY: `params` and `blob` outlive the activation, which is waited for below.
    // Windows before 10 version 2004 has no process loopback and fails here.
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const blob),
            &handler,
        )
    }
    .map_err(|_| Missing::Unsupported)?;
    done.recv_timeout(ANSWER_WITHIN).map_err(|_| Missing::Failed)?;
    let failed = |_: windows::core::Error| Missing::Failed;
    let mut result = HRESULT(0);
    let mut unknown: Option<IUnknown> = None;
    // SAFETY: the activation has completed.
    unsafe { operation.GetActivateResult(&mut result, &mut unknown) }.map_err(failed)?;
    result.ok().map_err(|_| Missing::Unsupported)?;
    let client: IAudioClient = unknown.ok_or(Missing::Failed)?.cast().map_err(failed)?;
    // Floats, stereo, at the call's rate: Windows converts whatever plays.
    let format = WAVEFORMATEX {
        wFormatTag: FLOAT,
        nChannels: 2,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    // SAFETY: setting up and starting the client Windows just made.
    unsafe {
        // 200 ms of room, in 100 ns units.
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &format, None).map_err(failed)?;
        let event = CreateEventW(None, false, false, None).map_err(failed)?;
        let capture = client.SetEventHandle(event).and_then(|()| client.GetService::<IAudioCaptureClient>());
        let capture = match capture.and_then(|c| client.Start().map(|()| c)) {
            Ok(capture) => capture,
            Err(_) => {
                let _ = CloseHandle(event);
                return Err(Missing::Failed);
            }
        };
        Ok(Started { client, capture, event })
    }
}

/// Hands each packet over, as mono, until stopped or the client fails.
fn run(started: &Started, stop: &AtomicBool, push: &Push) {
    let mut mono = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: the client's own event, open until `open`'s thread closes it.
        unsafe { WaitForSingleObject(started.event, 100) };
        loop {
            // SAFETY: reading packets from the started client, each released after.
            let Ok(waiting) = (unsafe { started.capture.GetNextPacketSize() }) else { return };
            if waiting == 0 {
                break;
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if unsafe { started.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err() {
                return;
            }
            mono.clear();
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                mono.resize(frames as usize, 0.0);
            } else {
                // SAFETY: the buffer holds `frames` frames of the format asked for.
                let samples = unsafe { std::slice::from_raw_parts(data.cast::<f32>(), frames as usize * 2) };
                mono.extend(samples.as_chunks::<2>().0.iter().map(|[l, r]| (l + r) * 0.5));
            }
            let _ = unsafe { started.capture.ReleaseBuffer(frames) };
            push(&mono);
        }
    }
}
