use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use trib_engine::GraphEngine;

use crate::backend::{
    AudioBackend, AudioError, InputDeviceInfo, InputStreamStatus, OpenInput, StreamConfig,
    StreamHandle,
};

/// Deviceless backend: drives the engine at block cadence from a plain
/// thread, feeding a generated test tone as mono "input" and discarding
/// output. Serves dev machines without audio hardware/headers and the
/// daemon's integration tests — the whole engine path runs except the
/// speaker.
pub struct FakeBackend {
    pub tone_hz: f32,
    /// Linear amplitude of the tone (0.1 ≈ -20 dBFS: healthy green meters).
    pub tone_amplitude: f32,
}

impl Default for FakeBackend {
    fn default() -> Self {
        FakeBackend {
            tone_hz: 440.0,
            tone_amplitude: 0.1,
        }
    }
}

struct FakeStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The one "device" this backend has: the tone, openable at offset 0
    /// only (its samples are hardwired to flat channel 0).
    opened: Mutex<Option<OpenInput>>,
}

impl StreamHandle for FakeStream {
    fn open_input(&self, req: OpenInput) -> Result<(), AudioError> {
        if req.device.is_some() {
            return Err(AudioError::Device(
                "the fake backend has only the test tone".into(),
            ));
        }
        if req.offset != 0 {
            return Err(AudioError::Device("the fake tone lives at offset 0".into()));
        }
        *self.opened.lock().expect("fake open lock") = Some(req);
        Ok(())
    }

    fn close_input(&self, device: Option<&str>) -> Result<(), AudioError> {
        if device.is_none() {
            *self.opened.lock().expect("fake open lock") = None;
        }
        Ok(())
    }

    fn input_status(&self) -> Vec<InputStreamStatus> {
        self.opened
            .lock()
            .expect("fake open lock")
            .as_ref()
            .map(|req| InputStreamStatus {
                device: None,
                offset: req.offset,
                channels: req.channels,
                failed: false,
                underruns: 0,
                overruns: 0,
            })
            .into_iter()
            .collect()
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl AudioBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn input_devices(&self) -> Vec<InputDeviceInfo> {
        vec![InputDeviceInfo {
            name: "Test tone (fake backend)".into(),
            description: None,
            channels: 1,
            active: true,
            pulse: false,
            card: None,
            channel_map: None,
            muted: false,
            volume_percent: None,
        }]
    }

    fn start(
        &self,
        config: &StreamConfig,
        mut engine: GraphEngine,
    ) -> Result<Box<dyn StreamHandle>, AudioError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let StreamConfig {
            sample_rate,
            block_size,
        } = *config;
        let block_duration = Duration::from_secs_f64(block_size as f64 / sample_rate as f64);
        let phase_step = self.tone_hz / sample_rate as f32 * std::f32::consts::TAU;
        let amplitude = self.tone_amplitude;

        let thread = std::thread::Builder::new()
            .name("trib-fake-audio".into())
            .spawn(move || {
                let mut input = vec![0.0f32; block_size];
                let mut output = vec![0.0f32; block_size * 2];
                let mut phase = 0.0f32;
                while !stop_flag.load(Ordering::Relaxed) {
                    for sample in input.iter_mut() {
                        *sample = amplitude * phase.sin();
                        phase = (phase + phase_step) % std::f32::consts::TAU;
                    }
                    engine.process(&input, 1, &mut output);
                    std::thread::sleep(block_duration);
                }
            })
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        Ok(Box::new(FakeStream {
            stop,
            thread: Some(thread),
            opened: Mutex::new(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use trib_core::{InputAssign, MixerState, StripId, StripState};
    use trib_engine::{InputSlots, compile, engine_pair};

    use super::*;

    #[test]
    fn fake_stream_produces_meter_blocks_and_stops_on_drop() {
        let mut strip = StripState::new(StripId(0), "Ch 1".into());
        strip.input = Some(InputAssign {
            device: None,
            device_channel: 0,
        });
        let state = MixerState {
            strips: vec![strip],
            ..MixerState::default()
        };
        let compiled = compile(&state, 48_000, 256, &InputSlots::single_default(1));
        let (mut handle, engine) = engine_pair(compiled.graph);
        let config = StreamConfig {
            sample_rate: 48_000,
            block_size: 256,
        };
        let stream = FakeBackend::default().start(&config, engine).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(stream);
        let block = handle.meter_rx.pop().expect("meter blocks were pushed");
        assert_eq!(block.count, 2, "strip meter + master");
        assert!(block.peak[0] > 0.05, "the tone registered pre-fader");
    }
}
